"""Synthetic sessions for #971 condition 26: a replay fixture of made-up coding work.

Nothing here comes from a real session. Usage: synth.py OUT.jsonl [TURNS_A TURNS_B]
"""
import datetime
import json
import sys

ROOT = "__OBOETE_REPLAY_ROOT__"
THEMES = [
    ("station flag", "station", "add a --station flag to the tides command so a port can be chosen by its id"),
    ("harmonic loop", "harmonic", "fix the off-by-one in the loop over harmonic constituents"),
    ("metadata cache", "cache", "keep station metadata in a file under the cache directory"),
    ("datum offset", "datum", "apply the chart datum offset before heights are printed"),
    ("time zone", "zone", "print times in the station's own time zone, not UTC"),
    ("json output", "json", "add --format json beside the table output"),
    ("slack water", "slack", "mark slack water between a high and a low"),
    ("moon phase", "moon", "show the moon phase on each day of the table"),
    ("retry fetch", "fetch", "retry the constituent download three times with a pause"),
    ("units flag", "units", "add --units feet beside metres"),
    ("range summary", "range", "print the day's tidal range under each day"),
    ("config file", "config", "read defaults from tidepool.toml in the home directory"),
    ("leap second", "leap", "stop the epoch conversion from drifting across a leap second"),
    ("station search", "search", "find stations by a part of their name"),
    ("sunrise column", "sun", "add sunrise and sunset columns to the table"),
    ("csv export", "csv", "write the table as CSV with a header row"),
    ("spring neap", "spring", "label spring and neap tides in the week view"),
    ("offline mode", "offline", "run from the cache alone when there is no network"),
    ("height rounding", "round", "round heights to two decimals in every output"),
    ("progress bar", "progress", "show progress while a year of predictions is computed"),
]


def code(word, turn, lines):
    out = [f"//! tidepool: {word} support (part {turn}).", "use crate::model::{Station, Height, Instant};", ""]
    for i in range(lines):
        n = turn * 37 + i * 11
        out.append(f"/// Step {i} of the {word} path: folds constituent {n % 37} into the running height.")
        out.append(f"pub fn {word}_step_{i}(station: &Station, at: Instant, acc: Height) -> Height {{")
        out.append(f"    let speed = station.constituents[{n % 37}].speed_deg_per_hour * {1 + n % 7}.0;")
        out.append(f"    let phase = (at.hours_since_epoch() * speed + {n % 360}.0).to_radians();")
        out.append(f"    acc + station.constituents[{n % 37}].amplitude_m * phase.cos() // term {n}")
        out.append("}")
        out.append("")
    return "\n".join(out)


def tests(word, turn, n, failed=None):
    out = [f"running {n} tests"]
    for i in range(n):
        state = "FAILED" if failed == i else "ok"
        out.append(f"test {word}::case_{turn}_{i}_{['low', 'high', 'slack', 'ebb'][i % 4]}_water ... {state}")
    if failed is not None:
        out.append("")
        out.append(f"---- {word}::case_{turn}_{failed} stdout ----")
        out.append(f"assertion `left == right` failed: height at step {failed}")
        out.append(f"  left: {1.20 + turn / 10:.2f}")
        out.append(f" right: {1.25 + turn / 10:.2f}")
        out.append(f"test result: FAILED. {n - 1} passed; 1 failed; finished in 0.{40 + turn}s")
    else:
        out.append(f"test result: ok. {n} passed; 0 failed; finished in 0.{40 + turn}s")
    return "\n".join(out)


def grep_hits(word, turn, n):
    return "\n".join(
        f"src/{['table', 'predict', 'cli', 'model', 'fetch'][i % 5]}.rs:{20 + i * 7 + turn}:    "
        f"let {word}_{i} = table.rows[{i}].{word}_value(); // looked at for part {turn}"
        for i in range(n)
    )


def session(sid, first, turns, at, lines):
    def emit(event, payload, tags=None):
        nonlocal at
        at += datetime.timedelta(seconds=17)
        base = {"session_id": sid, "transcript_path": f"{ROOT}/{sid}.jsonl", "cwd": ROOT,
                "permission_mode": "default", "hook_event_name": event}
        base.update(payload)
        lines.append({"seq": len(lines), "agent": "claude", "event": event, "session": sid,
                      "ts": at.strftime("%Y-%m-%dT%H:%M:%S.000Z"), "payload": base})

    emit("SessionStart", {"source": "startup"})
    for t in range(first, first + turns):
        title, word, ask = THEMES[t % len(THEMES)]
        pid = f"p-{sid}-{t}"
        path = f"{ROOT}/src/{word}.rs"

        def tool(name, tool_input, response, i, agent=None):
            p = {"prompt_id": pid, "tool_use_id": f"tu-{sid}-{t}-{i}", "tool_name": name,
                 "tool_input": tool_input, "tool_response": response, "duration_ms": 40 + i}
            if agent:
                p["agent_id"] = agent
            emit("PostToolUse", p)

        emit("UserPromptSubmit", {"prompt_id": pid, "prompt":
             f"In tidepool, {ask}. Keep the table output as it is for anyone who passes no new option, "
             f"and add a test named {word}::case_{t}_0 that fails before the change."})
        body = code(word, t, 6)
        tool("Read", {"file_path": path},
             {"type": "text", "file": {"filePath": path, "content": body, "numLines": body.count("\n") + 1,
                                       "startLine": 1, "totalLines": body.count("\n") + 1}}, 0)
        tool("Grep", {"pattern": f"{word}_value", "path": f"{ROOT}/src"},
             {"mode": "content", "numFiles": 5, "content": grep_hits(word, t, 40)}, 1, agent=f"sub-{sid}-{t}")
        tool("Bash", {"command": f"cargo test {word}:: 2>&1 | tail -40", "description": f"run the {title} tests"},
             {"stdout": tests(word, t, 14, failed=0), "stderr": "", "interrupted": False}, 2)
        old = f"    acc + station.constituents[{(t * 37) % 37}].amplitude_m * phase.cos() // term {t * 37}"
        new = (f"    // {title}: the term is taken at the station's own offset, so case_{t}_0 holds.\n"
               f"    acc + station.constituents[{(t * 37) % 37}].amplitude_m * (phase - station.{word}_offset()).cos()")
        tool("Edit", {"file_path": path, "old_string": old, "new_string": new},
             {"filePath": path, "oldString": old, "newString": new, "replaceAll": False}, 3)
        tool("Bash", {"command": f"cargo test {word}:: 2>&1 | tail -40", "description": f"run the {title} tests again"},
             {"stdout": tests(word, t, 14), "stderr": "", "interrupted": False}, 4)
        emit("Stop", {"prompt_id": pid, "stop_hook_active": False, "last_assistant_message":
             f"Done: {ask}. The cause was in src/{word}.rs, where the term for the first constituent was taken "
             f"without the station's {word} offset; {word}_step_0 now subtracts it. {word}::case_{t}_0 failed "
             f"before the change (left {1.20 + t / 10:.2f}, right {1.25 + t / 10:.2f}) and passes after it; "
             f"all 14 tests of the module pass. The table output without a new option is unchanged. "
             f"Not done: the same offset in the week view, which still reads the old value."})
    emit("SessionEnd", {"reason": "prompt_input_exit"})
    return at


def main():
    out = sys.argv[1]
    a, b = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (12, 6)
    # Hours ago, so no session counts as work still going on.
    at = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=5)
    lines = []
    at = session("synth-a-0001", 0, a, at, lines)
    at = session("synth-b-0002", a, b, at + datetime.timedelta(minutes=9), lines)
    with open(out, "w", encoding="utf-8") as f:
        for line in lines:
            f.write(json.dumps(line) + "\n")
    chars = sum(len(json.dumps(line["payload"])) for line in lines)
    print(f"{len(lines)} events, {chars} payload characters, about {chars * 28 // 100} estimated tokens")


if __name__ == "__main__":
    main()
