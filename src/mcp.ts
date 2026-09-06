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
import { resolveRepoIdentity } from './repo-identity.js';

/** The legacy-era revisions this server speaks; the last one is what an unknown client gets. */
const PROTOCOL_VERSIONS = ['2024-11-05', '2025-03-26', '2025-06-18', '2025-11-25'] as const;
const LATEST_PROTOCOL = PROTOCOL_VERSIONS[PROTOCOL_VERSIONS.length - 1];
const DEFAULT_LIMIT = 10;
const MAX_LIMIT = 50;
const DATABASE_TIMEOUT_MS = 2_000;

const limitSchema = z.number().int().min(1).max(MAX_LIMIT);

export const MCP_TOOLS = [
  {
    name: 'search',
    description: 'Search memories of the current repository',
    inputSchema: {
      type: 'object',
      properties: {
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
      properties: { id: { type: 'string' } },
      required: ['id'],
    },
  },
] as const;

const MAX_TEXT = 4096;
const MAX_LINE_CHARS = 1_048_576;

const toolArguments = {
  search: z.looseObject({ query: z.string().max(MAX_TEXT), limit: limitSchema.default(DEFAULT_LIMIT) }),
  timeline: z.looseObject({ session: z.string().max(MAX_TEXT).optional(), limit: limitSchema.default(DEFAULT_LIMIT) }),
  get: z.looseObject({ id: z.string().max(MAX_TEXT) }),
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

function withDatabase<T>(paths: OboetePaths, fn: (db: DatabaseSync) => T): T {
  const opened = openDatabase({ path: paths.db, timeoutMs: DATABASE_TIMEOUT_MS });
  try {
    return fn(opened.db);
  } finally {
    opened.db.close();
  }
}

function textResult(text: string, structuredContent: unknown): ToolResult {
  return { content: [{ type: 'text', text }], structuredContent };
}

function callTool(
  name: string,
  rawArguments: unknown,
  context: { repoId: string; paths: OboetePaths },
): ToolResult {
  // contracts/mcp.md: the boundary is the working directory, so a `repo` argument is refused.
  if (typeof rawArguments === 'object' && rawArguments !== null && 'repo' in rawArguments) {
    throw invalidParams('the repository is derived from the working directory the server was started in; a repo argument is not accepted');
  }
  const schema = toolArguments[name as keyof typeof toolArguments];
  if (schema === undefined) throw invalidParams(`unknown tool: ${name}`);
  const parsed = schema.safeParse(rawArguments ?? {});
  if (!parsed.success) throw invalidParams(z.prettifyError(parsed.error));
  const args = parsed.data;

  return withDatabase(context.paths, (db) => {
    const scope = memoryScope(db, { repoId: context.repoId, destination: 'injection' });
    switch (name) {
      case 'search': {
        const search = args as z.infer<typeof toolArguments.search>;
        const rows = searchMemories(db, { ...context, query: search.query, limit: search.limit });
        const memories = rows.map((row) => {
          const memory = getMemory(db, row.id, scope);
          return {
            ...row,
            citations: memorySources(db, row.id).flatMap((source) =>
              source.citation_value === null ? [] : [source.citation_value],
            ),
            stale: memory?.citations_ok === 0,
          };
        });
        return memories.length === 0
          ? textResult(`${EMPTY_REASON}\n${LEXICAL_NOTE}`, { memories, degraded: null, note: LEXICAL_NOTE })
          : textResult(renderSearch(rows), { memories, degraded: null });
      }
      case 'timeline': {
        const options = args as z.infer<typeof toolArguments.timeline>;
        const sessions = timeline(db, context.repoId, {
          ...(options.session === undefined ? {} : { sessionId: options.session }),
          limit: options.limit,
        });
        return textResult(
          sessions.length === 0 ? 'No sessions were found in the current repository.' : renderTimeline(sessions),
          { sessions },
        );
      }
      default: {
        const { id } = args as z.infer<typeof toolArguments.get>;
        const memory = getMemory(db, id, scope);
        if (memory === null) return { content: [{ type: 'text', text: 'not found' }], isError: true };
        const sources = memorySources(db, memory.id);
        return textResult(
          `Memory ${memory.id} is a ${memory.type} titled ${JSON.stringify(memory.title ?? '(untitled)')}. ` +
            `Its body is ${JSON.stringify(memory.body ?? '')}. Its sensitivity is ${memory.sensitivity}.`,
          { ...memory, sources },
        );
      }
    }
  });
}

function handle(
  method: string,
  params: unknown,
  context: { repoId: string; paths: OboetePaths },
): unknown {
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

function serveLine(line: string, runtime: McpRuntime, context: { repoId: string; paths: OboetePaths }): void {
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
    const result = handle(method, params, context);
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
  const context = { repoId: resolveRepoIdentity(runtime.cwd).id, paths };

  for await (const line of createInterface({ input: runtime.input, crlfDelay: Number.POSITIVE_INFINITY })) {
    if (line.trim() === '') continue;
    serveLine(line, runtime, context);
  }
  return 0;
}
