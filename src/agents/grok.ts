// Grok Build hook payloads. Shapes verified by the fixtures under test/contracts/grok/ and the R13
// probes: every field exists twice, camelCase (Grok's own) and snake_case (the Claude compatibility
// layer), and only the camelCase set is read; `write` returns `toolResult.type = "SearchReplace"`
// so the result shape is chosen by `toolName`; `run_terminal_command.output` is a byte array and
// `output_for_prompt` is the string; a failed shell call arrives as PostToolUse with `exit_code`;
// `PermissionDenied` fires only for a permission-rule deny and carries no reason; `Stop` carries
// `lastAssistantMessage` on `end_turn`; `PostCompact` has no summary but a per-compaction
// `timestamp`.
import { normalizeToolName, type NormalizedEvent, type ToolInput, type ToolName } from '../events.js';
import {
  asRecord,
  buildEnvelope,
  capPaths,
  capText,
  capToolText,
  countLines,
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

// Each Grok result shape has exactly one field oboete stores; a result without it stores nothing,
// because every other field of these shapes is content no adapter named (contracts/agents.md).
function editsOutput(response: unknown): string {
  const applied = readRecord(asRecord(response), 'EditsApplied');
  return capText(readContent(applied, 'tool_output_for_prompt') ?? '');
}

const GROK_TOOLS: Record<string, ToolMapping> = {
  read_file: {
    input: (raw): ToolInput => ({ paths: capPaths([readString(raw, 'target_file')]) }),
    output: (response) =>
      capText(readContent(readRecord(asRecord(response), 'FileContent'), 'content') ?? ''),
  },
  write: {
    input: (raw): ToolInput => {
      const content = readContent(raw, 'content') ?? '';
      return {
        paths: capPaths([readString(raw, 'file_path')]),
        text: capToolText(content),
        lines_added: countLines(content),
      };
    },
    output: editsOutput,
  },
  search_replace: {
    input: (raw): ToolInput => {
      const before = readContent(raw, 'old_string') ?? '';
      const after = readContent(raw, 'new_string') ?? '';
      return {
        paths: capPaths([readString(raw, 'file_path')]),
        text: capToolText(`${before}\n→\n${after}`),
        lines_added: countLines(after),
        lines_removed: countLines(before),
      };
    },
    output: editsOutput,
  },
  run_terminal_command: {
    input: (raw): ToolInput => ({ paths: [], command: capText(readContent(raw, 'command') ?? '') }),
    // `output` is a byte array; only the prompt string is text and only it is stored.
    output: (response) => capText(readContent(asRecord(response), 'output_for_prompt') ?? ''),
  },
};

function grokTool(native: string, toolName: ToolName): ToolMapping | undefined {
  // The native name comes from the payload, so an inherited key is not a mapping (FR-004).
  if (Object.hasOwn(GROK_TOOLS, native)) return GROK_TOOLS[native];
  // Grok spells an MCP tool `<server>__<tool>` (R13 probe: `oboete_probe__search`).
  return toolName.startsWith('mcp:') ? genericTool() : undefined;
}

/** A shell call that exits non-zero arrives as PostToolUse, so the exit code decides is_error. */
function failed(result: Record<string, unknown> | null): boolean {
  if (result === null || !Object.hasOwn(result, 'exit_code')) return false;
  const code = result.exit_code;
  return typeof code === 'number' && code !== 0;
}

export function adaptGrok(input: AdapterInput): AdapterOutput {
  const payload = asRecord(input.payload);
  if (payload === null) return metadataOnly(input, 'payload_invalid');
  const envelope = buildEnvelope(input, {
    sessionId: readString(payload, 'sessionId'),
    cwd: readString(payload, 'cwd'),
  });
  if (envelope === null) return metadataOnly(input, 'payload_invalid');
  // Tool events carry no promptId (R13 probe); Stop and UserPromptSubmit do.
  const turn = promptRef(readString(payload, 'promptId'));

  switch (input.eventName) {
    case 'SessionStart': {
      const source = readString(payload, 'source');
      // `new` is Grok's word for a fresh headless session. `load` covers both resume and
      // `--fork-session`, which differ only by the session id, so capture's conversation policy
      // decides between them (R13 probe 2026-09-03).
      if (source !== 'new' && source !== 'load') return metadataOnly(input, 'payload_invalid');
      return toEvents([
        { ...envelope, kind: 'session_start', source: source === 'new' ? 'startup' : 'resume' },
      ]);
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
      const native = readString(payload, 'toolName');
      const callId = readString(payload, 'toolUseId');
      if (native === undefined || callId === undefined) return metadataOnly(input, 'payload_invalid');
      const toolName = normalizeToolName('grok', native);
      const mapping = grokTool(native, toolName);
      // Until a fixture describes the tool, only its metadata is kept (R13 row 1).
      if (mapping === undefined) return metadataOnly(input, 'unmapped_payload', native);
      const toolInput = mapping.input(readRecord(payload, 'toolInput'));
      if (input.eventName === 'PreToolUse') {
        return toEvents([
          {
            ...envelope,
            ...turn,
            kind: 'tool_call',
            tool_call_id: callId,
            tool_name_native: native,
            tool_name: toolName,
            input: toolInput,
          },
        ]);
      }
      const result = readRecord(payload, 'toolResult');
      return toEvents(
        [
          {
            ...envelope,
            ...turn,
            kind: 'tool_result',
            tool_call_id: callId,
            output: mapping.output(result),
            is_error: failed(result),
          },
        ],
        // The result is the body of the file the call named, so the path rules must see that path
        // even though the result event has no path field (FR-017, R4).
        toolInput.paths,
      );
    }
    case 'PostToolUseFailure': {
      const callId = readString(payload, 'toolUseId');
      const error = readContent(payload, 'error');
      if (callId === undefined || error === undefined) return metadataOnly(input, 'payload_invalid');
      return toEvents([
        { ...envelope, ...turn, kind: 'tool_failure', tool_call_id: callId, error: capText(error) },
      ]);
    }
    case 'PermissionDenied': {
      const callId = readString(payload, 'toolUseId');
      if (callId === undefined) return metadataOnly(input, 'payload_invalid');
      return toEvents([
        {
          ...envelope,
          ...turn,
          kind: 'tool_failure',
          tool_call_id: callId,
          // The payload carries no reason field, and the event fires only for a permission-rule
          // deny, so the text states that and nothing more (R13 probe 2026-09-03).
          error: 'permission denied by a permission rule',
        },
      ]);
    }
    case 'Stop': {
      const reason = readString(payload, 'reason');
      // A second Stop fires at session end with `shutdown` or `channel_closed`; recording it would
      // add a ghost turn with no prompt (contracts/agents.md Grok row).
      if (reason !== 'end_turn') return metadataOnly(input, 'event_not_captured');
      const message = readContent(payload, 'lastAssistantMessage');
      const events: NormalizedEvent[] = [];
      if (message !== undefined) {
        events.push({ ...envelope, ...turn, kind: 'last_assistant_message', text: capText(message) });
      }
      // The turn ordinal belongs to capture, which counts turns per session (R7).
      events.push({ ...envelope, ...turn, kind: 'turn_end', turn_index: 0, reason });
      return toEvents(events);
    }
    case 'PostCompact':
      return toEvents([
        {
          ...envelope,
          kind: 'compaction_summary',
          // No summary field by contract; the nanosecond timestamp is the verified per-compaction
          // key (R13 probe 2026-09-03).
          text: '',
          compaction_key: capText(readContent(payload, 'timestamp') ?? ''),
        },
      ]);
    case 'SessionEnd':
      return toEvents([
        { ...envelope, kind: 'session_end', reason: capText(readContent(payload, 'reason') ?? '') },
      ]);
    default:
      return metadataOnly(input, 'event_not_captured');
  }
}
