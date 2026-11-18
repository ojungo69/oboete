"""Work-view spike, pass item 4: the work-view snapshot shows the same requests as /api/my-work.

Usage: compare.py <dir with api_my_work_range_week.json and api_work_view.json>
Checks the count, each title, and each lifecycle status against table 5.2 of PLAN.md.
"""
import json
import pathlib
import sys

d = pathlib.Path(sys.argv[1])
work = json.loads((d / "api_my_work_range_week.json").read_text(encoding="utf-8"))
view = json.loads((d / "api_work_view.json").read_text(encoding="utf-8"))
TABLE = {"needs": "needs-you", "ready": "needs-review", "working": "in-progress", "blocked": "blocked",
         "quiet": "quiet", "todo": "todo", "unknown": "unknown", "closed": "closed", "shipped": "shipped"}
mine = {r["id"]: r for r in work.get("requests", [])}
shown = {r["id"]: r for r in view.get("humanRequests", [])}
bad = []
if len(mine) != len(shown):
    bad.append(f"count {len(mine)} != {len(shown)}")
for i, r in mine.items():
    s = shown.get(i)
    if s is None:
        bad.append(f"missing {i[:40]}")
        continue
    if (r.get("title") or None) != (s.get("title") or None):
        bad.append(f"title {i[:30]}")
    if TABLE.get(r.get("status"), "unknown") != s["lifecycle"]["status"]:
        bad.append(f"status {i[:30]} {r.get('status')} -> {s['lifecycle']['status']}")
    for k in s["conversationKeys"]:
        if len(json.loads(k)) < 3:
            bad.append(f"short key {i[:30]}")
statuses = {}
for r in mine.values():
    statuses[r.get("status")] = statuses.get(r.get("status"), 0) + 1
agents = {a["platformSource"] for a in view.get("agents", [])}
keyed = {json.loads(k)[1] for r in shown.values() for k in r["conversationKeys"]}
print(json.dumps({"requests": len(mine), "statuses": statuses, "areas": len(work.get("areas", [])),
                  "agents": len(view.get("agents", [])), "platforms": sorted(agents),
                  "keys_without_agent_platform": sorted(keyed - agents), "mismatches": bad[:20]}))
