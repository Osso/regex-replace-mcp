import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CliRequest, CliRunner, ReplaceResult } from "./regex-replace-tool.ts";

export interface ExecOptions {
  cwd: string;
  signal?: AbortSignal;
}

export interface ExecResult {
  code: number;
  stdout: string;
  stderr: string;
}

export type ExecCommand = (
  command: string,
  args: string[],
  options: ExecOptions,
) => Promise<ExecResult>;

export function createJsonCliRunner(binaryPath: string, exec: ExecCommand): CliRunner {
  return async (request, signal) => {
    const directory = await mkdtemp(join(tmpdir(), "regex-replace-"));
    const requestPath = join(directory, "request.json");
    try {
      await writeFile(requestPath, JSON.stringify(request), "utf8");
      const result = await exec(binaryPath, [requestPath], {
        cwd: request.cwd,
        signal: request.action === "plan" ? signal : undefined,
      });
      if (result.code !== 0) {
        const message = result.stderr.trim() || `regex-replace-json exited with code ${result.code}`;
        throw new Error(message);
      }
      return parseReplaceResult(result.stdout);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  };
}

function parseReplaceResult(output: string): ReplaceResult {
  const parsed: unknown = JSON.parse(output);
  if (!isReplaceResult(parsed)) {
    throw new Error("regex-replace-json returned an invalid result");
  }
  return parsed;
}

function isReplaceResult(value: unknown): value is ReplaceResult {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const result = value as Partial<ReplaceResult>;
  return (
    typeof result.dryRun === "boolean" &&
    typeof result.planHash === "string" &&
    typeof result.matchedFiles === "number" &&
    typeof result.totalReplacements === "number" &&
    typeof result.filesModified === "number" &&
    Array.isArray(result.changes)
  );
}
