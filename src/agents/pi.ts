// Pi extension events, as the detached capture child of T051 receives them. Pi has no hook process:
// the extension serializes the event it subscribed to into oboete's own envelope, which is defined
// here so that the child and the adapter cannot drift apart. Shapes verified by the fixtures under
// test/contracts/pi/ and the R13 probes: `tool_result` carries `input`, `content`, `isError` (and
// `details` for edit) and is the single subscription point, `session_start.reason` is always
// `startup`, and `session_compact.compactionEntry.id` is the per-compaction key.
import { z } from 'zod';
import {
  normalizeToolName,
  type InputSource,
  type NormalizedEvent,
  type ToolInput,
} from '../events.js';
import {
  asRecord,
  buildEnvelope,
  capPaths,
  capText,
  capToolText,
  countLines,
  describeToolOutput,
  metadataOnly,
  promptRef,
  readContent,
  readRecord,
  readString,
  toEvents,
  type AdapterInput,
  type AdapterOutput,
} from './index.js';

/**
 * What the Pi capture child writes to its own stdin reader. `prompt_id` is an identifier the
 * extension generates in memory on every `input` event and repeats on every later event of that
 * turn, so Pi events get the per-turn identity key that Claude Code's `prompt_id` gives: without
 * one, `eventIdKey` falls back to the content hash and two identical turn ends of one session
 * collapse into a single row (R7).
 */
export const piEnvelopeSchema = z.strictObject({
  event: z.string().min(1),
  session_id: z.string().min(1),
  cwd: z.string().min(1),
  model: z.string().min(1).optional(),
  prompt_id: z.string().min(1).optional(),
  payload: z.unknown(),
});

export type PiEnvelope = z.infer<typeof piEnvelopeSchema>;

function readArray(source: Record<string, unknown> | null, key: string): unknown[] {
  if (source === null || !Object.hasOwn(source, key)) return [];
  const value = source[key];
  return Array.isArray(value) ? value : [];
}

const PI_TOOLS: Record<string, (raw: Record<string, unknown> | null) => ToolInput> = {
  read: (raw) => ({ paths: capPaths([readString(raw, 'path')]) }),
  write: (raw) => {
    const content = readContent(raw, 'content') ?? '';
    return {
      paths: capPaths([readString(raw, 'path')]),
      text: capToolText(content),
      lines_added: countLines(content),
    };
  },
  edit: (raw) => {
    const blocks: string[] = [];
    let added = 0;
    let removed = 0;
    for (const item of readArray(raw, 'edits')) {
      const edit = asRecord(item);
      const before = readContent(edit, 'oldText') ?? '';
      const after = readContent(edit, 'newText') ?? '';
      blocks.push(`${before}\n→\n${after}`);
      added += countLines(after);
      removed += countLines(before);
    }
    return {
      paths: capPaths([readString(raw, 'path')]),
      text: capToolText(blocks.join('\n')),
      lines_added: added,
      lines_removed: removed,
    };
  },
  bash: (raw) => ({ paths: [], command: capText(readContent(raw, 'command') ?? '') }),
};

/** Pi returns a tool result as content blocks; only the text of a block is stored. */
function blockText(blocks: unknown): string {
  if (!Array.isArray(blocks)) return describeToolOutput(blocks);
  const texts: string[] = [];
  for (const block of blocks) {
    const text = readContent(asRecord(block), 'text');
    if (text !== undefined) texts.push(text);
  }
  return capText(texts.join('\n'));
}

function inputSource(source: string | undefined): InputSource {
  // Pi's own vocabulary is interactive | rpc | extension; interactive is a person typing.
  if (source === 'rpc') return 'rpc';
  if (source === 'extension') return 'extension';
  return 'user';
}

export function adaptPi(input: AdapterInput): AdapterOutput {
  const wire = piEnvelopeSchema.safeParse(input.payload);
  if (!wire.success) return metadataOnly(input, 'payload_invalid');
  const envelope = buildEnvelope(input, {
    sessionId: wire.data.session_id,
    cwd: wire.data.cwd,
    model: wire.data.model,
  });
  if (envelope === null) return metadataOnly(input, 'payload_invalid');
  const payload = asRecord(wire.data.payload);
  const turn = promptRef(wire.data.prompt_id);

  switch (input.eventName) {
    case 'session_start':
      // `reason` is `startup` for a start, a resume and a fork alike, so a resume is recognized by
      // session id continuity and never by this field (R13 probe 2026-09-03).
      return toEvents([{ ...envelope, ...turn, kind: 'session_start', source: 'startup' }]);
    case 'input': {
      const text = readContent(payload, 'text');
      if (text === undefined) return metadataOnly(input, 'payload_invalid');
      return toEvents([
        {
          ...envelope,
          ...turn,
          kind: 'prompt',
          text: capText(text),
          input_source: inputSource(readString(payload, 'source')),
        },
      ]);
    }
    case 'tool_result': {
      const native = readString(payload, 'toolName');
      const callId = readString(payload, 'toolCallId');
      if (native === undefined || callId === undefined) return metadataOnly(input, 'payload_invalid');
      // Until a fixture describes the tool, only its metadata is kept (R13 row 1). Pi has no
      // verified MCP naming, so an unlisted name is never read as a server tool either.
      if (!Object.hasOwn(PI_TOOLS, native)) return metadataOnly(input, 'unmapped_payload', native);
      const mapping = PI_TOOLS[native];
      if (mapping === undefined) return metadataOnly(input, 'unmapped_payload', native);
      // Pi delivers the call and its result in one event, so both normalized events are built here.
      return toEvents([
        {
          ...envelope,
          ...turn,
          kind: 'tool_call',
          tool_call_id: callId,
          tool_name_native: native,
          tool_name: normalizeToolName('pi', native),
          input: mapping(readRecord(payload, 'input')),
        },
        {
          ...envelope,
          ...turn,
          kind: 'tool_result',
          tool_call_id: callId,
          output: blockText(payload?.content),
          is_error: payload?.isError === true,
        },
      ]);
    }
    case 'agent_settled': {
      const message = readContent(payload, 'text');
      const events: NormalizedEvent[] = [];
      if (message !== undefined && message !== '') {
        events.push({ ...envelope, ...turn, kind: 'last_assistant_message', text: capText(message) });
      }
      // The turn ordinal belongs to capture, which counts turns per session (R7).
      events.push({ ...envelope, ...turn, kind: 'turn_end', turn_index: 0, reason: 'agent_settled' });
      return toEvents(events);
    }
    case 'session_shutdown':
      return toEvents([
        { ...envelope, ...turn, kind: 'session_end', reason: capText(readContent(payload, 'reason') ?? '') },
      ]);
    case 'session_compact': {
      const entry = readRecord(payload, 'compactionEntry');
      const key = readString(entry, 'id');
      if (key === undefined) return metadataOnly(input, 'payload_invalid');
      return toEvents([
        {
          ...envelope,
          ...turn,
          kind: 'compaction_summary',
          text: capText(readContent(entry, 'summary') ?? ''),
          compaction_key: capText(key),
        },
      ]);
    }
    default:
      return metadataOnly(input, 'event_not_captured');
  }
}
