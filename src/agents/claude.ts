// Claude Code hook payloads. Shapes verified by the fixtures under test/contracts/claude/ and the
// R13 probe notes in docs/research/oboete-contracts-probes.md: `tool_input` is snake_case,
// `tool_response` is camelCase with a different shape per tool, a failed call arrives as
// `PostToolUseFailure` only (never as `PostToolUse`) with `error` as a string, `Stop` carries
// `last_assistant_message`, and `PostCompact` carries `compact_summary`.
import {
  normalizeToolName,
  type NormalizedEvent,
  type SessionStartSource,
  type ToolName,
} from '../events.js';
import {
  asRecord,
  buildEnvelope,
  capPaths,
  capText,
  capToolText,
  countLines,
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

// Sources Claude Code is verified to send (R13 probe: `--resume` gives `resume`, `--fork-session`
// gives `fork`). A value outside this set is not guessed.
const CLAUDE_SOURCES: SessionStartSource[] = ['startup', 'resume', 'clear', 'compact', 'fork'];

// Tools whose input and output shape a fixture describes, plus the three whose input is a plain
// argument object. Anything else is stored metadata-only (R13 row 1).
function writtenPath(response: unknown): string {
  // Write and Edit echo the whole file back; the path alone is what a summary needs.
  const path = readString(asRecord(response), 'filePath');
  return path === undefined ? describeToolOutput(response) : capText(path);
}

const CLAUDE_TOOLS: Record<string, ToolMapping> = {
  Read: {
    input: (raw) => ({ paths: capPaths([readString(raw, 'file_path')]) }),
    output: (response) => {
      const content = readContent(readRecord(asRecord(response), 'file'), 'content');
      return content === undefined ? describeToolOutput(response) : capText(content);
    },
  },
  Write: {
    input: (raw) => {
      const content = readContent(raw, 'content') ?? '';
      return {
        paths: capPaths([readString(raw, 'file_path')]),
        text: capToolText(content),
        lines_added: countLines(content),
      };
    },
    output: writtenPath,
  },
  Edit: {
    input: (raw) => {
      const before = readContent(raw, 'old_string') ?? '';
      const after = readContent(raw, 'new_string') ?? '';
      return {
        paths: capPaths([readString(raw, 'file_path')]),
        text: capToolText(`${before}\n→\n${after}`),
        lines_added: countLines(after),
        lines_removed: countLines(before),
      };
    },
    output: writtenPath,
  },
  Bash: {
    input: (raw) => ({ paths: [], command: capText(readContent(raw, 'command') ?? '') }),
    output: (response) => {
      const record = asRecord(response);
      const stdout = readContent(record, 'stdout');
      const stderr = readContent(record, 'stderr');
      if (stdout === undefined && stderr === undefined) return describeToolOutput(response);
      return capText([stdout, stderr].filter((part) => part !== undefined && part !== '').join('\n'));
    },
  },
  Grep: genericTool(),
  Glob: genericTool(),
  Task: genericTool(),
};

function claudeTool(native: string, toolName: ToolName): ToolMapping | undefined {
  // The native name comes from the payload, so an inherited key is not a mapping (FR-004).
  if (Object.hasOwn(CLAUDE_TOOLS, native)) return CLAUDE_TOOLS[native];
  // An MCP tool has no fixture of its own; its arguments are kept as JSON, which the detector
  // scans like any other captured text before the first write (FR-018).
  return toolName.startsWith('mcp:') ? genericTool() : undefined;
}

export function adaptClaude(input: AdapterInput): AdapterOutput {
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
  const turn = promptRef(readString(payload, 'prompt_id'));

  switch (input.eventName) {
    case 'SessionStart': {
      const source = CLAUDE_SOURCES.find((known) => known === readString(payload, 'source'));
      // A source outside the verified set is not guessed: reading a fork as a startup would join
      // two conversations (contracts/agents.md "Event identity and conversation identity").
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
      const toolName = normalizeToolName('claude', native);
      const mapping = claudeTool(native, toolName);
      // Until a fixture describes the tool, only its metadata is kept (R13 row 1).
      if (mapping === undefined) return metadataOnly(input, 'unmapped_payload', native);
      const toolInput = mapping.input(readRecord(payload, 'tool_input'));
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
      return toEvents(
        [
          {
            ...envelope,
            ...turn,
            kind: 'tool_result',
            tool_call_id: callId,
            output: mapping.output(payload.tool_response),
            // A failed call never reaches PostToolUse on Claude Code (R13 probe 2026-09-03).
            is_error: false,
          },
        ],
        // The result is the body of the file the call named, so the path rules must see that path
        // even though the result event has no path field (FR-017, R4).
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
    case 'Stop': {
      const message = readContent(payload, 'last_assistant_message');
      const events: NormalizedEvent[] = [];
      if (message !== undefined) {
        events.push({ ...envelope, ...turn, kind: 'last_assistant_message', text: capText(message) });
      }
      // The turn ordinal belongs to capture, which counts turns per session; the adapter has no
      // counter, which is why the event id of a turn_end uses `prompt_id` when there is one (R7).
      events.push({ ...envelope, ...turn, kind: 'turn_end', turn_index: 0, reason: 'stop' });
      return toEvents(events);
    }
    case 'PostCompact':
      return toEvents([
        {
          ...envelope,
          ...turn,
          kind: 'compaction_summary',
          text: capText(readContent(payload, 'compact_summary') ?? ''),
          // Claude Code has no per-compaction id, so the event id becomes the epoch key; the
          // prompt id above is what keeps two compactions of one session apart (A16, R7).
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
