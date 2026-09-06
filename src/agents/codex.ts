// Codex CLI hook payloads. Shapes verified by the fixtures under test/contracts/codex/ and the R13
// probe notes: Codex has no read tool (reads arrive as `Bash` commands), writes and edits arrive as
// `apply_patch` whose paths live only inside the patch text, `tool_response` is a bare string,
// `PostCompact` carries no summary field, and `/new` fires no SessionStart at all (A18).
import {
  normalizeToolName,
  type SessionStartSource,
  type ToolInput,
  type ToolName,
} from '../events.js';
import {
  asRecord,
  buildEnvelope,
  capPaths,
  capText,
  capToolText,
  describeToolOutput,
  genericTool,
  metadataOnly,
  promptRef,
  readContent,
  readRecord,
  readString,
  toEvents,
  type AdapterInput,
  type AdapterOutput,
  type ToolMapping,
} from './index.js';

// The schema enum of the SessionStart input; `clear` is in it but was never observed (A18).
const CODEX_SOURCES: SessionStartSource[] = ['startup', 'resume', 'clear', 'compact'];

const PATCH_FILE = /^\*\*\* (Add|Update|Delete) File: (.+)$/;

type Patch = { paths: string[]; added: number; removed: number; addOnly: boolean };

/** The path of an `apply_patch` call exists only in its patch text, so it is read from there. */
function parsePatch(patch: string): Patch {
  const paths: string[] = [];
  let added = 0;
  let removed = 0;
  let sections = 0;
  let additions = 0;
  for (const line of patch.split('\n')) {
    const file = PATCH_FILE.exec(line);
    if (file !== null) {
      sections += 1;
      if (file[1] === 'Add') additions += 1;
      paths.push((file[2] ?? '').trim());
      continue;
    }
    // Inside a hunk; the `*** …` and `@@` lines start with neither character.
    if (line.startsWith('+')) added += 1;
    else if (line.startsWith('-')) removed += 1;
  }
  return { paths, added, removed, addOnly: sections > 0 && additions === sections };
}

function patchOf(raw: Record<string, unknown> | null): Patch {
  return parsePatch(readContent(raw, 'command') ?? '');
}

/** Codex returns the tool output as a bare string, not as an object (fixtures, R13 row 1). */
function bareString(response: unknown): string {
  return typeof response === 'string' ? capText(response) : describeToolOutput(response);
}

const CODEX_TOOLS: Record<string, ToolMapping> = {
  Bash: {
    input: (raw): ToolInput => ({ paths: [], command: capText(readContent(raw, 'command') ?? '') }),
    output: bareString,
  },
  apply_patch: {
    input: (raw): ToolInput => {
      const patch = readContent(raw, 'command') ?? '';
      const parsed = parsePatch(patch);
      return {
        paths: capPaths(parsed.paths),
        text: capToolText(patch),
        lines_added: parsed.added,
        lines_removed: parsed.removed,
      };
    },
    output: bareString,
    // A patch that only adds files wrote them; anything else edited an existing file.
    name: (raw): ToolName => (patchOf(raw).addOnly ? 'write' : 'edit'),
  },
};

function codexTool(native: string, toolName: ToolName): ToolMapping | undefined {
  // The native name comes from the payload, so an inherited key is not a mapping (FR-004).
  if (Object.hasOwn(CODEX_TOOLS, native)) return CODEX_TOOLS[native];
  return toolName.startsWith('mcp:') ? genericTool() : undefined;
}

export function adaptCodex(input: AdapterInput): AdapterOutput {
  const payload = asRecord(input.payload);
  if (payload === null) return metadataOnly(input, 'payload_invalid');
  const envelope = buildEnvelope(input, {
    sessionId: readString(payload, 'session_id'),
    cwd: readString(payload, 'cwd'),
    model: readString(payload, 'model'),
    agentId: readString(payload, 'agent_id'),
    agentType: readString(payload, 'agent_type'),
  });
  if (envelope === null) return metadataOnly(input, 'payload_invalid');
  // Codex supplies no prompt id; `turn_id` is its per-turn identity and is what keeps two turns of
  // one session apart in the event id (R7, contracts/agents.md "Event identity").
  const turn = promptRef(readString(payload, 'turn_id'));

  switch (input.eventName) {
    case 'SessionStart': {
      const source = CODEX_SOURCES.find((known) => known === readString(payload, 'source'));
      if (source === undefined) return metadataOnly(input, 'payload_invalid');
      return toEvents([{ ...envelope, ...turn, kind: 'session_start', source }]);
    }
    case 'UserPromptSubmit': {
      const prompt = readContent(payload, 'prompt');
      if (prompt === undefined) return metadataOnly(input, 'payload_invalid');
      return toEvents([
        { ...envelope, ...turn, kind: 'prompt', text: capText(prompt), input_source: 'user' },
      ]);
    }
    case 'PreToolUse':
    case 'PostToolUse': {
      const native = readString(payload, 'tool_name');
      const callId = readString(payload, 'tool_use_id');
      if (native === undefined || callId === undefined) return metadataOnly(input, 'payload_invalid');
      const mapping = codexTool(native, normalizeToolName('codex', native));
      // Until a fixture describes the tool, only its metadata is kept (R13 row 1).
      if (mapping === undefined) return metadataOnly(input, 'unmapped_payload', native);
      const raw = readRecord(payload, 'tool_input');
      const toolInput = mapping.input(raw);
      if (input.eventName === 'PreToolUse') {
        return toEvents([
          {
            ...envelope,
            ...turn,
            kind: 'tool_call',
            tool_call_id: callId,
            tool_name_native: native,
            tool_name: mapping.name?.(raw) ?? normalizeToolName('codex', native),
            input: toolInput,
          },
        ]);
      }
      return toEvents(
        [
          {
            ...envelope,
            ...turn,
            kind: 'tool_result',
            tool_call_id: callId,
            output: mapping.output(payload.tool_response),
            is_error: false,
          },
        ],
        // The paths a patch names live only in the call, so the result would otherwise reach the
        // path rules without them (FR-017, R4).
        toolInput.paths,
      );
    }
    case 'PostToolUseFailure': {
      const callId = readString(payload, 'tool_use_id');
      const error = readContent(payload, 'error');
      if (callId === undefined || error === undefined) return metadataOnly(input, 'payload_invalid');
      return toEvents([
        { ...envelope, ...turn, kind: 'tool_failure', tool_call_id: callId, error: capText(error) },
      ]);
    }
    case 'Stop':
      // Codex hands the final assistant text to `codex exec --output-last-message`, not to the Stop
      // hook, whose documented input carries no message field, so the turn end is all there is.
      return toEvents([{ ...envelope, ...turn, kind: 'turn_end', turn_index: 0, reason: 'stop' }]);
    case 'PostCompact':
      return toEvents([
        {
          ...envelope,
          ...turn,
          kind: 'compaction_summary',
          // No summary field and no per-compaction id by contract, so the event id is the epoch
          // key and the turn id above keeps two compactions apart (R13 probe 2026-09-03, A16).
          text: '',
          compaction_key: '',
        },
      ]);
    case 'SessionEnd':
      return toEvents([
        { ...envelope, ...turn, kind: 'session_end', reason: capText(readContent(payload, 'reason') ?? '') },
      ]);
    default:
      return metadataOnly(input, 'event_not_captured');
  }
}
