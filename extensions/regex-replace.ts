import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  formatSize,
  truncateHead,
  withFileMutationQueue,
  type ExtensionAPI,
  type ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import { Text } from "@earendil-works/pi-tui";
import { Type, type Static } from "typebox";
import { createJsonCliRunner } from "../src/pi/json-cli-runner.ts";
import {
  OperationMutex,
  runRegexReplace,
  type ReplaceResult,
} from "../src/pi/regex-replace-tool.ts";

const extensionDirectory = dirname(fileURLToPath(import.meta.url));
const binaryPath = resolve(extensionDirectory, "../target/release/regex-replace-json");
const operationMutex = new OperationMutex();
// Leave room for the truncation notice below agent-core's 10 KiB result cap.
const MODEL_OUTPUT_MAX_BYTES = 8 * 1024;
const MODEL_OUTPUT_MAX_LINES = 1000;
// Pi replaces serialized tool details above 10 KiB with truncation metadata.
const RENDER_DETAILS_MAX_BYTES = 8 * 1024;
const RENDER_DETAILS_TRUNCATION_NOTICE = "[Diff truncated in tool details.]";

const regexReplaceSchema = Type.Object(
  {
    files: Type.String({ description: "Gitignore-aware glob relative to the current working directory" }),
    pattern: Type.String({ description: "Pattern using Rust regex syntax" }),
    replacement: Type.String({ description: "Replacement using $1, $2, and $0 capture references" }),
    expectedMatches: Type.Integer({ minimum: 1, description: "Exact number of matches required" }),
    dryRun: Type.Optional(Type.Boolean({ description: "Preview without writing (default: false)" })),
    maxFiles: Type.Optional(Type.Integer({ minimum: 1, description: "Maximum matched files (default: 100)" })),
    maxTotalBytes: Type.Optional(
      Type.Integer({ minimum: 1, description: "Maximum total input bytes (default: 10 MiB)" }),
    ),
    maxMatches: Type.Optional(
      Type.Integer({ minimum: 1, description: "Maximum replacements (default: 10000)" }),
    ),
  },
  { additionalProperties: false },
);

type RegexReplaceInput = Static<typeof regexReplaceSchema>;

interface RegexReplaceDetails {
  dryRun: boolean;
  totalReplacements: number;
  filesModified: number;
  diff: string;
}

type RegexReplaceToolDefinition = ToolDefinition<
  typeof regexReplaceSchema,
  RegexReplaceDetails
> & {
  approvalRequired: true;
};

export interface RegexReplaceExtensionApi {
  exec: ExtensionAPI["exec"];
  registerTool(tool: RegexReplaceToolDefinition): void;
}

export default function regexReplaceExtension(pi: RegexReplaceExtensionApi): void {
  const runCli = createJsonCliRunner(binaryPath, (command, args, options) =>
    pi.exec(command, args, options),
  );

  const tool = {
    name: "regex_replace",
    label: "Regex Replace",
    description:
      "Safely replace a Rust regex across gitignore-aware files with an exact expected match count, dry-run diffs, stale-plan protection, and bounded scope.",
    promptSnippet: "Regex replace across files with exact expected match counts and unified diffs",
    promptGuidelines: [
      "Use regex_replace for multi-file regex changes; use edit for exact single-file text replacements.",
      "Call regex_replace with dryRun=true first when the expected match count is uncertain.",
      "Do not pass maxFiles, maxTotalBytes, or maxMatches unless the user explicitly requests a non-default limit.",
    ],
    approvalRequired: true,
    parameters: regexReplaceSchema,

    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      if (signal?.aborted) {
        throw new Error("Regex replacement cancelled before planning");
      }
      const result = await runRegexReplace(
        params as RegexReplaceInput,
        ctx.cwd,
        runCli,
        withFileMutationQueue,
        operationMutex,
        signal,
      );
      const diff = result.changes.map((change) => change.diff).join("\n");
      return {
        content: [{ type: "text", text: formatModelOutput(result, diff) }],
        details: buildRenderDetails(result, diff),
      };
    },

    renderCall(args, theme) {
      const mode = args.dryRun ? "preview" : "apply";
      const text = `${theme.fg("toolTitle", theme.bold("regex_replace "))}${theme.fg("accent", args.files)} ${theme.fg("dim", `/${args.pattern}/ expected=${args.expectedMatches} ${mode}`)}`;
      return new Text(text, 0, 0);
    },

    renderResult(result, { expanded, isPartial }, theme) {
      if (isPartial) {
        return new Text(theme.fg("warning", "Planning regex replacement..."), 0, 0);
      }
      const details = result.details;
      if (!isRegexReplaceDetails(details)) {
        return new Text(theme.fg("error", "Invalid replacement details"), 0, 0);
      }
      const mode = details.dryRun ? "Previewed" : "Applied";
      let text = theme.fg(
        "success",
        `${mode} ${details.totalReplacements} replacement${pluralSuffix(details.totalReplacements)} in ${details.filesModified} file${pluralSuffix(details.filesModified)}`,
      );
      if (expanded && details.diff) {
        text += `\n${colorDiff(details.diff, theme)}`;
      }
      return new Text(text, 0, 0);
    },
  } satisfies RegexReplaceToolDefinition;

  pi.registerTool(tool);
}

function buildRenderDetails(result: ReplaceResult, diff: string): RegexReplaceDetails {
  const summary = {
    dryRun: result.dryRun,
    totalReplacements: result.totalReplacements,
    filesModified: result.filesModified,
  };
  const fullDetails = { ...summary, diff };
  if (serializedDetailsBytes(fullDetails) <= RENDER_DETAILS_MAX_BYTES) {
    return fullDetails;
  }

  let boundedDiff = "";
  for (const line of diff.split("\n")) {
    const candidate = boundedDiff ? `${boundedDiff}\n${line}` : line;
    const truncatedDiff = `${candidate}\n${RENDER_DETAILS_TRUNCATION_NOTICE}`;
    if (serializedDetailsBytes({ ...summary, diff: truncatedDiff }) > RENDER_DETAILS_MAX_BYTES) {
      break;
    }
    boundedDiff = candidate;
  }

  const truncatedDiff = boundedDiff
    ? `${boundedDiff}\n${RENDER_DETAILS_TRUNCATION_NOTICE}`
    : RENDER_DETAILS_TRUNCATION_NOTICE;
  return { ...summary, diff: truncatedDiff };
}

function serializedDetailsBytes(details: RegexReplaceDetails): number {
  return Buffer.byteLength(JSON.stringify(details), "utf8");
}

function formatModelOutput(result: ReplaceResult, diff: string): string {
  const mode = result.dryRun ? "Previewed" : "Applied";
  const summary = `${mode} ${result.totalReplacements} replacement${pluralSuffix(result.totalReplacements)} in ${result.filesModified} file${pluralSuffix(result.filesModified)}. Plan: ${result.planHash}`;
  const fullOutput = diff ? `${summary}\n\n${diff}` : summary;
  const truncation = truncateHead(fullOutput, {
    maxBytes: MODEL_OUTPUT_MAX_BYTES,
    maxLines: MODEL_OUTPUT_MAX_LINES,
  });
  if (!truncation.truncated) {
    return truncation.content;
  }
  return `${truncation.content}\n\n[Diff truncated: ${truncation.outputLines}/${truncation.totalLines} lines, ${formatSize(truncation.outputBytes)}/${formatSize(truncation.totalBytes)}.]`;
}

function colorDiff(diff: string, theme: { fg(color: string, text: string): string }): string {
  return diff
    .split("\n")
    .map((line) => {
      if (line.startsWith("+") && !line.startsWith("+++")) {
        return theme.fg("success", line);
      }
      if (line.startsWith("-") && !line.startsWith("---")) {
        return theme.fg("error", line);
      }
      return theme.fg("dim", line);
    })
    .join("\n");
}

function isRegexReplaceDetails(value: unknown): value is RegexReplaceDetails {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const details = value as Partial<RegexReplaceDetails>;
  return (
    typeof details.dryRun === "boolean" &&
    typeof details.totalReplacements === "number" &&
    typeof details.filesModified === "number" &&
    typeof details.diff === "string"
  );
}

function pluralSuffix(count: number): string {
  return count === 1 ? "" : "s";
}
