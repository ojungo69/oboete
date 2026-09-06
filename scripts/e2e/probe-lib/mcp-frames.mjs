import fs from "node:fs";

export function readMcpFrames(file) {
  const frames = [];
  if (fs.existsSync(file)) {
    for (const l of fs.readFileSync(file, "utf8").split("\n").filter(Boolean)) {
      try {
        frames.push(JSON.parse(l));
      } catch {
        frames.push({ parse_error: l.slice(0, 120) });
      }
    }
  }
  const methods = [];
  let protocolVersion = null;
  for (const f of frames) {
    const msg = f.frame || f;
    if (f.dir === "in" && msg.method === "initialize") protocolVersion = msg.params?.protocolVersion || null;
    if (f.dir === "in" && msg.method) methods.push(msg.method);
  }
  return {
    frames,
    methods,
    protocolVersion,
    hasList: methods.includes("tools/list"),
    hasCall: methods.includes("tools/call"),
  };
}
