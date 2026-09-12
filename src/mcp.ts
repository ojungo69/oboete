// `oboete mcp`: the tool surface of contracts/mcp.md over stdio, newline-delimited JSON-RPC 2.0,
// legacy-era lifecycle only. Security-owned (research R12): the repository is the server's own
// working directory, derived with the same function as capture, and every read goes through the
// injection scope of db/queries.ts, so an agent's tool call sees exactly what its packs see.
import { createInterface } from 'node:readline';
import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';
import { z } from 'zod';

import { openDatabase } from './db/open.js';
import { getMemory, memoryScope, memorySources, timeline } from './db/queries.js';
import {
  EMPTY_REASON,
  LEXICAL_NOTE,
  renderSearch,
  renderTimeline,
  searchMemories,
} from './memories-cli.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from './paths.js';
import { syncStatus } from './sync/status.js';
import { resolveRepoIdentity } from './repo-identity.js';
import { chooseSourceWork, chooseWork, readWorkSelection, workStatus } from './work.js';
import { filterMemoryOutput, filterReadOutput, filterTimelineOutput } from './privacy/provenance.js';
import { sharingStatus } from './sharing.js';

/** The legacy-era revisions this server speaks; the last one is what an unknown client gets. */
const PROTOCOL_VERSIONS = ['2024-11-05', '2025-03-26', '2025-06-18', '2025-11-25'] as const;
const LATEST_PROTOCOL = PROTOCOL_VERSIONS.at(-1) as (typeof PROTOCOL_VERSIONS)[number];
const DEFAULT_LIMIT = 10;
const MAX_LIMIT = 50;
const DATABASE_TIMEOUT_MS = 2_000;

const limitSchema = z.number().int().min(1).max(MAX_LIMIT);
const readProperties = {
  binding: { type: 'string', minLength: 1, maxLength: 128, description: 'An exact current-worktree binding ID' },
  history: { type: 'boolean', default: false, description: 'Deliberately include retained historical memories and checkpoints' },
} as const;

export const MCP_TOOLS = [
  {
    name: 'search',
    description: 'Search memories of the current repository',
    inputSchema: {
      type: 'object',
      properties: {
        ...readProperties,
        query: { type: 'string' },
        limit: { type: 'integer', minimum: 1, maximum: MAX_LIMIT, default: DEFAULT_LIMIT },
      },
      required: ['query'],
    },
  },
  {
    name: 'timeline',
    description: 'Sessions and turns of the current repository',
    inputSchema: {
      type: 'object',
      properties: {
        ...readProperties,
        session: { type: 'string' },
        limit: { type: 'integer', minimum: 1, maximum: MAX_LIMIT, default: DEFAULT_LIMIT },
      },
    },
  },
  {
    name: 'get',
    description: 'One memory by id within the current repository',
    inputSchema: {
      type: 'object',
      properties: { ...readProperties, id: { type: 'string' } },
      required: ['id'],
    },
  },
  {
    name: 'work_status',
    description: 'List work and selection bindings in the current worktree; optionally include retained work elsewhere in this repository',
    annotations: { readOnlyHint: true, idempotentHint: true, openWorldHint: false },
    inputSchema: { type: 'object', properties: { all: { type: 'boolean', default: false } }, additionalProperties: false },
  },
  {
    name: 'work_choose',
    description: 'Select work for an exact current-worktree binding or explicitly assign one unbound historical source',
    annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
    inputSchema: {
      type: 'object', properties: {
        binding: { type: 'string', minLength: 1, maxLength: 128 },
        source: { type: 'string', minLength: 1, maxLength: 128 },
        work: { type: 'string', minLength: 1, maxLength: 128, description: 'A work ID or new' },
      },
      oneOf: [{ required: ['binding', 'work'], not: { required: ['source'] } },
        { required: ['source', 'work'], not: { required: ['binding'] } }],
      additionalProperties: false,
    },
  },
  {
    name: 'sharing_status',
    description: 'Inspect sharing proposals originating in the current repository. Approve or reject through the human-operated CLI or viewer.',
    annotations: { readOnlyHint: true, idempotentHint: true, openWorldHint: false },
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'sync_status',
    description: 'Report the local device-sync state: configured space, replicas seen, open conflicts and withheld rows. Push, pull and resolve run only through the human-operated CLI.',
    annotations: { readOnlyHint: true, idempotentHint: true, openWorldHint: false },
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
] as const;

const MAX_TEXT = 4096;
const MAX_LINE_CHARS = 1_048_576;
const readArguments = { binding: z.string().min(1).max(128).optional(), history: z.boolean().default(false) };

const toolArguments = {
  sharing_status: z.strictObject({}),
  sync_status: z.strictObject({}),
  search: z.looseObject({ ...readArguments, query: z.string().max(MAX_TEXT), limit: limitSchema.default(DEFAULT_LIMIT) }),
  timeline: z.looseObject({ ...readArguments, session: z.string().max(MAX_TEXT).optional(), limit: limitSchema.default(DEFAULT_LIMIT) }),
  get: z.looseObject({ ...readArguments, id: z.string().max(MAX_TEXT) }),
  work_status: z.strictObject({ all: z.boolean().default(false) }),
  work_choose: z.union([
    z.strictObject({ binding: z.string().min(1).max(128), work: z.string().min(1).max(128) }),
    z.strictObject({ source: z.string().min(1).max(128), work: z.string().min(1).max(128) }),
  ]),
};

const requestSchema = z.looseObject({
  jsonrpc: z.literal('2.0'),
  id: z.union([z.string(), z.number(), z.null()]).optional(),
  method: z.string(),
  params: z.unknown().optional(),
});

export type McpRuntime = {
  cwd: string;
  input: NodeJS.ReadableStream;
  writeOut(text: string): void;
  writeError(text: string): void;
};

type JsonRpcId = string | number | null;
type McpContext = { repoId: string; contextKey: string | null; repoRoot: string; paths: OboetePaths };
type ToolResult = {
  content: { type: 'text'; text: string }[];
  structuredContent?: unknown;
  isError?: true;
};

class RpcError extends Error {
  constructor(
    readonly code: number,
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
  }
}

const invalidParams = (data: unknown): RpcError => new RpcError(-32602, 'Invalid params', data);

function runtimeWith(overrides: Partial<McpRuntime>): McpRuntime {
  return {
    cwd: process.cwd(),
    input: process.stdin,
    writeOut: (text) => {
      process.stdout.write(text);
    },
    writeError: (text) => {
      process.stderr.write(text);
    },
    ...overrides,
  };
}

async function withDatabase<T>(paths: OboetePaths, fn: (db: DatabaseSync) => T | Promise<T>): Promise<T> {
  const opened = openDatabase({ path: paths.db, timeoutMs: DATABASE_TIMEOUT_MS });
  try {
    return await fn(opened.db);
  } finally {
    opened.db.close();
  }
}

function textResult(text: string, structuredContent: unknown): ToolResult {
  return { content: [{ type: 'text', text }], structuredContent };
}

function currentContext(context: McpContext): boolean {
  const current = resolveRepoIdentity(context.repoRoot);
  return current.id === context.repoId && current.root === context.repoRoot && current.worktreeKey !== null
    && current.worktreeKey === context.contextKey;
}

async function callTool(
  name: string,
  rawArguments: unknown,
  context: McpContext,
): Promise<ToolResult> {
  // contracts/mcp.md: the boundary is the working directory, so a `repo` argument is refused.
  if (typeof rawArguments === 'object' && rawArguments !== null && 'repo' in rawArguments) {
    throw invalidParams('the repository is derived from the working directory the server was started in; a repo argument is not accepted');
  }
  const schema = Object.hasOwn(toolArguments, name) ? toolArguments[name as keyof typeof toolArguments] : undefined;
  if (schema === undefined) throw invalidParams(`unknown tool: ${name}`);
  const parsed = schema.safeParse(rawArguments ?? {});
  if (!parsed.success) throw invalidParams(z.prettifyError(parsed.error));
  const args: Record<string, unknown> = parsed.data;
  const unavailable: ToolResult = { content: [{ type: 'text', text: 'The repository context changed or could not be verified. Start a new MCP session in the intended directory.' }], isError: true };
  if (!currentContext(context)) return unavailable;

  const result: ToolResult = await withDatabase(context.paths, async (db) => {
    const selection = readWorkSelection(db, { ...context,
      bindingId: 'binding' in args && typeof args.binding === 'string' ? args.binding : undefined });
    const scope = memoryScope(db, { repoId: context.repoId, destination: 'injection',
      workId: selection.workId, history: 'history' in args && args.history === true });
    const privacy = { ...context, home: context.paths.home, bindingId: selection.bindingId,
      workId: selection.workId, history: 'history' in args && args.history === true };
    switch (name) {
      case 'sharing_status': {
        const status = await sharingStatus(db, privacy);
        const text = status.proposals.length === 0 ? 'No sharing proposals are available in this repository.'
          : status.proposals.map((proposal) => `${proposal.id}: ${proposal.state} ${JSON.stringify(proposal.candidate_title)}: ${JSON.stringify(proposal.candidate_body)}`).join('\n');
        return textResult(text + (status.hasMore ? '\nMore proposals are available. Review these to see the next ones.' : ''), status);
      }
      case 'sync_status': {
        const status = syncStatus(db, context.paths);
        return textResult(JSON.stringify(status), status);
      }
      case 'work_status': {
        const status = workStatus(db, context, (args as z.infer<typeof toolArguments.work_status>).all);
        const checked = await filterReadOutput(db, { ...privacy, history: true },
          status.works.flatMap((work) => work.checkpoint === null ? [] : [work.checkpoint]), status.works);
        const visible = new Map(checked.memories.map((memory) => [memory.id, memory]));
        const result = { ...status, works: checked.works.map((work) => ({ ...work,
          checkpoint: work.checkpoint === null ? null : visible.get(work.checkpoint.id) ?? null })) };
        return textResult(JSON.stringify(result), result);
      }
      case 'work_choose': {
        const choice = args as z.infer<typeof toolArguments.work_choose>;
        const selected = 'binding' in choice
          ? chooseWork(db, { ...context, bindingId: choice.binding, workId: choice.work, now: Date.now() })
          : chooseSourceWork(db, { ...context, root: context.repoRoot, sourceId: choice.source, workId: choice.work, now: Date.now() });
        return selected === null ? { content: [{ type: 'text',
          text: 'The work or binding was not found in the current scope, or the choice is no longer current.' }], isError: true }
          : textResult('The work selection was saved.', { selection: selected });
      }
      case 'search': {
        const search = args as z.infer<typeof toolArguments.search>;
        const rows = searchMemories(db, { ...context, query: search.query, limit: search.limit,
          workId: selection.workId, history: search.history });
        const checked = await filterReadOutput(db, privacy, rows.map((row) => {
          const memory = getMemory(db, row.id, scope);
          return {
            ...row,
            citations: memorySources(db, row.id).flatMap((source) =>
              source.citation_value === null ? [] : [source.citation_value],
            ),
            stale: memory?.citations_ok === 0,
          };
        }), selection.choices);
        const memories = checked.memories;
        selection.choices = checked.works;
        return memories.length === 0
          ? textResult(`${EMPTY_REASON}\n${LEXICAL_NOTE}`, { memories, degraded: null, note: LEXICAL_NOTE, selection })
          : textResult(renderSearch(memories), { memories, degraded: null, selection });
      }
      case 'timeline': {
        const options = args as z.infer<typeof toolArguments.timeline>;
        const checked = await filterTimelineOutput(db, privacy, timeline(db, context.repoId, {
          ...(options.session === undefined ? {} : { sessionId: options.session }),
          limit: options.limit,
          workId: selection.workId, history: options.history,
        }), selection.choices);
        const sessions = checked.sessions;
        selection.choices = checked.works;
        return textResult(
          sessions.length === 0 ? 'No sessions were found in the current repository.' : renderTimeline(sessions),
          { sessions, selection },
        );
      }
      default: {
        const { id } = args as z.infer<typeof toolArguments.get>;
        const memory = getMemory(db, id, scope);
        if (memory === null) return { content: [{ type: 'text', text: 'not found' }], isError: true };
        const sources = memorySources(db, memory.id);
        const visible = (await filterMemoryOutput(db, privacy, [{ ...memory, sources }]))[0];
        if (visible === undefined) {
          return { content: [{ type: 'text', text: 'not found' }], isError: true };
        }
        return textResult(
          `Memory ${memory.id} is a ${memory.type} titled ${JSON.stringify(memory.title ?? '(untitled)')}. ` +
            `Its body is ${JSON.stringify(memory.body ?? '')}. Its sensitivity is ${memory.sensitivity}.`,
          visible,
        );
      }
    }
  });
  return name === 'work_choose' || currentContext(context) ? result : unavailable;
}

async function handle(
  method: string,
  params: unknown,
  context: McpContext,
): Promise<unknown> {
  switch (method) {
    case 'initialize': {
      const requested = (params as { protocolVersion?: unknown } | undefined)?.protocolVersion;
      const protocolVersion = PROTOCOL_VERSIONS.find((version) => version === requested) ?? LATEST_PROTOCOL;
      return {
        protocolVersion,
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: 'oboete', version: OBOETE_VERSION },
      };
    }
    case 'ping':
      return {};
    case 'tools/list':
      return { tools: MCP_TOOLS };
    case 'tools/call': {
      const call = params as { name?: unknown; arguments?: unknown } | undefined;
      if (typeof call?.name !== 'string') throw invalidParams('tools/call needs a tool name');
      return callTool(call.name, call.arguments, context);
    }
    default:
      // `server/discover` and everything else the legacy era does not define (contracts/mcp.md).
      throw new RpcError(-32601, 'Method not found', method);
  }
}

function respond(runtime: McpRuntime, id: JsonRpcId, body: { result: unknown } | { error: RpcError }): void {
  const frame =
    'result' in body
      ? { jsonrpc: '2.0', id, result: body.result }
      : {
          jsonrpc: '2.0',
          id,
          error: {
            code: body.error.code,
            message: body.error.message,
            ...(body.error.data === undefined ? {} : { data: body.error.data }),
          },
        };
  runtime.writeOut(`${JSON.stringify(frame)}\n`);
}

async function serveLine(line: string, runtime: McpRuntime, context: McpContext): Promise<void> {
  if (line.length > MAX_LINE_CHARS) {
    // ponytail: readline still buffers the oversized line; the client spawns this server, so the cost is its own.
    respond(runtime, null, { error: new RpcError(-32600, 'Invalid Request') });
    return;
  }
  let message: unknown;
  try {
    message = JSON.parse(line);
  } catch {
    respond(runtime, null, { error: new RpcError(-32700, 'Parse error') });
    return;
  }
  const request = requestSchema.safeParse(message);
  if (!request.success) {
    const id = (message as { id?: JsonRpcId } | null)?.id ?? null;
    respond(runtime, typeof id === 'object' ? null : id, { error: new RpcError(-32600, 'Invalid Request') });
    return;
  }
  const { id, method, params } = request.data;
  const notification = id === undefined;
  try {
    const result = await handle(method, params, context);
    if (!notification) respond(runtime, id, { result });
  } catch (error) {
    if (notification) return;
    if (error instanceof RpcError) {
      respond(runtime, id, { error });
    } else {
      // The message can quote SQLite or a path, never captured content; the class name is enough.
      respond(runtime, id, { error: new RpcError(-32603, 'Internal error', error instanceof Error ? error.name : 'Error') });
    }
  }
}

/** `oboete mcp`: serves until stdin closes; exit 0, or 2 when given any argument. */
export async function runMcp(argv: string[], overrides: Partial<McpRuntime> = {}): Promise<number> {
  const runtime = runtimeWith(overrides);
  try {
    parseArgs({ args: argv, allowPositionals: false, strict: true, options: {} });
  } catch (error) {
    runtime.writeError(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  const identity = resolveRepoIdentity(runtime.cwd);
  const context = { repoId: identity.id, contextKey: identity.worktreeKey, repoRoot: identity.root, paths };

  for await (const line of createInterface({ input: runtime.input, crlfDelay: Number.POSITIVE_INFINITY })) {
    if (line.trim() === '') continue;
    await serveLine(line, runtime, context);
  }
  return 0;
}
