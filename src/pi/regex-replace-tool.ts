export interface RegexReplaceParams {
  files: string;
  pattern: string;
  replacement: string;
  expectedMatches: number;
  dryRun?: boolean;
  maxFiles?: number;
  maxTotalBytes?: number;
  maxMatches?: number;
}

interface BaseCliRequest {
  cwd: string;
  files: string;
  pattern: string;
  replacement: string;
  expectedMatches: number;
  maxFiles: number;
  maxTotalBytes: number;
  maxMatches: number;
}

export interface PlanCliRequest extends BaseCliRequest {
  action: "plan";
}

export interface ApplyCliRequest extends BaseCliRequest {
  action: "apply";
  planHash: string;
  targets: string[];
}

export type CliRequest = PlanCliRequest | ApplyCliRequest;

export interface LineChange {
  lineNumber: number;
  before: string;
  after: string;
}

export interface FileChange {
  path: string;
  absolutePath: string;
  originalHash: string;
  replacements: number;
  diff: string;
  lineChanges: LineChange[];
}

export interface ReplaceResult {
  dryRun: boolean;
  planHash: string;
  matchedFiles: number;
  totalReplacements: number;
  filesModified: number;
  changes: FileChange[];
}

export type CliRunner = (request: CliRequest, signal?: AbortSignal) => Promise<ReplaceResult>;
export type FileQueue = <T>(path: string, operation: () => Promise<T>) => Promise<T>;

const DEFAULT_MAX_FILES = 100;
const DEFAULT_MAX_TOTAL_BYTES = 10 * 1024 * 1024;
const DEFAULT_MAX_MATCHES = 10_000;

export class OperationMutex {
  private tail: Promise<void> = Promise.resolve();

  run<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.tail.then(operation, operation);
    this.tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

export async function runRegexReplace(
  params: RegexReplaceParams,
  cwd: string,
  runCli: CliRunner,
  queueFile: FileQueue,
  mutex: OperationMutex,
  signal?: AbortSignal,
): Promise<ReplaceResult> {
  return mutex.run(async () => {
    const planRequest = buildPlanRequest(params, cwd);
    const plan = await runCli(planRequest, signal);
    if (params.dryRun ?? false) {
      return plan;
    }

    const targets = sortedTargets(plan);
    const applyRequest: ApplyCliRequest = {
      ...planRequest,
      action: "apply",
      planHash: plan.planHash,
      targets,
    };
    return withFileQueues(targets, queueFile, () => runCli(applyRequest, signal));
  });
}

function buildPlanRequest(params: RegexReplaceParams, cwd: string): PlanCliRequest {
  return {
    action: "plan",
    cwd,
    files: params.files.startsWith("@") ? params.files.slice(1) : params.files,
    pattern: params.pattern,
    replacement: params.replacement,
    expectedMatches: params.expectedMatches,
    maxFiles: params.maxFiles ?? DEFAULT_MAX_FILES,
    maxTotalBytes: params.maxTotalBytes ?? DEFAULT_MAX_TOTAL_BYTES,
    maxMatches: params.maxMatches ?? DEFAULT_MAX_MATCHES,
  };
}

function sortedTargets(plan: ReplaceResult): string[] {
  const targets = plan.changes.map((change) => change.absolutePath);
  return [...new Set(targets)].sort();
}

function withFileQueues<T>(
  targets: string[],
  queueFile: FileQueue,
  operation: () => Promise<T>,
): Promise<T> {
  const acquire = (index: number): Promise<T> => {
    const target = targets[index];
    if (target === undefined) {
      return operation();
    }
    return queueFile(target, () => acquire(index + 1));
  };
  return acquire(0);
}
