import type { McpRpcOutcome } from "./rpc-client.js";

export function jsonContent(data: unknown) {
	return { content: [{ type: "text" as const, text: JSON.stringify(data) }] };
}

function finalizeDelivery(outcome: McpRpcOutcome, status: "handed_off" | "failed"): void {
	if (!outcome.ok) return;
	try {
		void outcome.finalizeDelivery?.(status).catch(() => {});
	} catch {
		// Retrieval diagnostics must never alter the MCP tool result.
	}
}

export function rpcContent(
	outcome: McpRpcOutcome,
	select: (result: Record<string, unknown>) => unknown = (result) => result,
) {
	if (!outcome.ok) return jsonContent({ error: outcome.error });
	try {
		const content = jsonContent(select(outcome.result));
		finalizeDelivery(outcome, "handed_off");
		return content;
	} catch (error) {
		finalizeDelivery(outcome, "failed");
		throw error;
	}
}
