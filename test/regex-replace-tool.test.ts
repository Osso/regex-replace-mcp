import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import {
  OperationMutex,
  runRegexReplace,
  type CliRequest,
  type FileQueue,
  type ReplaceResult,
} from "../src/pi/regex-replace-tool.ts";
import { createJsonCliRunner } from "../src/pi/json-cli-runner.ts";
import regexReplaceExtension, {
  type RegexReplaceExtensionApi,
} from "../extensions/regex-replace.ts";

const plan: ReplaceResult = {
  dryRun: true,
  planHash: "plan-123",
  matchedFiles: 2,
  totalReplacements: 2,
  filesModified: 2,
  changes: [
    {
      path: "b.txt",
      absolutePath: "/tmp/b.txt",
      originalHash: "b-hash",
      replacements: 1,
      diff: "--- b.txt\n+++ b.txt\n-old\n+new\n",
      lineChanges: [],
    },
    {
      path: "a.txt",
      absolutePath: "/tmp/a.txt",
      originalHash: "a-hash",
      replacements: 1,
      diff: "--- a.txt\n+++ a.txt\n-old\n+new\n",
      lineChanges: [],
    },
  ],
};

const params = {
  files: "@**/*.txt",
  pattern: "old",
  replacement: "new",
  expectedMatches: 2,
  dryRun: false,
  maxFiles: 10,
  maxTotalBytes: 1024,
  maxMatches: 10,
};

function immediateQueue(events: string[]): FileQueue {
  return async (path, operation) => {
    events.push(`enter:${path}`);
    try {
      return await operation();
    } finally {
      events.push(`exit:${path}`);
    }
  };
}

test("extension registers an approval-gated regex_replace tool", () => {
  let registeredName: string | undefined;
  let approvalRequired: boolean | undefined;
  let description: string | undefined;
  const api: RegexReplaceExtensionApi = {
    async exec() {
      throw new Error("registration must not execute the CLI");
    },
    registerTool(tool) {
      registeredName = tool.name;
      approvalRequired = tool.approvalRequired;
      description = tool.description;
    },
  };

  regexReplaceExtension(api);

  assert.equal(registeredName, "regex_replace");
  assert.equal(approvalRequired, true);
  assert.match(description ?? "", /expected match count/i);
});

test("dry-run plans once without acquiring mutation queues", async () => {
  const requests: CliRequest[] = [];
  const result = await runRegexReplace(
    { ...params, dryRun: true },
    "/workspace",
    async (request) => {
      requests.push(request);
      return plan;
    },
    async () => {
      throw new Error("dry-run must not queue files");
    },
    new OperationMutex(),
  );

  assert.equal(result, plan);
  assert.deepEqual(requests, [
    {
      action: "plan",
      cwd: "/workspace",
      files: "**/*.txt",
      pattern: "old",
      replacement: "new",
      expectedMatches: 2,
      maxFiles: 10,
      maxTotalBytes: 1024,
      maxMatches: 10,
    },
  ]);
});

test("apply locks sorted frozen targets and forwards the approved plan", async () => {
  const requests: CliRequest[] = [];
  const queueEvents: string[] = [];
  const applied = { ...plan, dryRun: false };
  const result = await runRegexReplace(
    params,
    "/workspace",
    async (request) => {
      requests.push(request);
      return request.action === "plan" ? plan : applied;
    },
    immediateQueue(queueEvents),
    new OperationMutex(),
  );

  assert.equal(result, applied);
  assert.deepEqual(queueEvents, [
    "enter:/tmp/a.txt",
    "enter:/tmp/b.txt",
    "exit:/tmp/b.txt",
    "exit:/tmp/a.txt",
  ]);
  assert.deepEqual(requests[1], {
    action: "apply",
    cwd: "/workspace",
    files: "**/*.txt",
    pattern: "old",
    replacement: "new",
    expectedMatches: 2,
    planHash: "plan-123",
    targets: ["/tmp/a.txt", "/tmp/b.txt"],
    maxFiles: 10,
    maxTotalBytes: 1024,
    maxMatches: 10,
  });
});

test("file queue prevents a built-in-style edit from overlapping apply", async () => {
  const events: string[] = [];
  const queue = serialFileQueue();
  let releaseApply = (): void => {
    throw new Error("apply gate was not initialized");
  };
  const applyGate = new Promise<void>((resolve) => {
    releaseApply = resolve;
  });
  let markApplyStarted = (): void => {
    throw new Error("apply-start signal was not initialized");
  };
  const started = new Promise<void>((resolve) => {
    markApplyStarted = resolve;
  });

  const replacePromise = runRegexReplace(
    { ...params, expectedMatches: 1 },
    "/workspace",
    async (request) => {
      if (request.action === "plan") {
        return { ...plan, totalReplacements: 1, filesModified: 1, changes: [plan.changes[1]] };
      }
      events.push("replace-start");
      markApplyStarted();
      await applyGate;
      events.push("replace-end");
      return { ...plan, dryRun: false };
    },
    queue,
    new OperationMutex(),
  );

  await started;
  const editPromise = queue("/tmp/a.txt", async () => {
    events.push("edit");
  });
  await Promise.resolve();
  assert.deepEqual(events, ["replace-start"]);
  releaseApply();
  await Promise.all([replacePromise, editPromise]);
  assert.deepEqual(events, ["replace-start", "replace-end", "edit"]);
});

test("JSON runner writes a temporary request file and parses CLI output", async () => {
  let requestPath = "";
  const runner = createJsonCliRunner(
    "/opt/regex-replace-json",
    async (command, args, options) => {
      requestPath = args[0] ?? "";
      assert.equal(command, "/opt/regex-replace-json");
      assert.equal(options.cwd, "/workspace");
      const requestJson = await readFile(requestPath, "utf8");
      const request = JSON.parse(requestJson);
      assert.equal(request.action, "plan");
      return { code: 0, stdout: JSON.stringify(plan), stderr: "" };
    },
  );

  const result = await runner(
    {
      action: "plan",
      cwd: "/workspace",
      files: "**/*.txt",
      pattern: "old",
      replacement: "new",
      expectedMatches: 2,
      maxFiles: 10,
      maxTotalBytes: 1024,
      maxMatches: 10,
    },
  );

  assert.deepEqual(result, plan);
  await assert.rejects(access(requestPath));
});

function serialFileQueue(): FileQueue {
  const tails = new Map<string, Promise<void>>();
  return async <T>(path: string, operation: () => Promise<T>): Promise<T> => {
    const previous = tails.get(path) ?? Promise.resolve();
    let release = (): void => {
      throw new Error("queue release was not initialized");
    };
    const current = new Promise<void>((resolve) => {
      release = resolve;
    });
    tails.set(path, previous.then(() => current));
    await previous;
    try {
      return await operation();
    } finally {
      release();
      if (tails.get(path) === current) {
        tails.delete(path);
      }
    }
  };
}
