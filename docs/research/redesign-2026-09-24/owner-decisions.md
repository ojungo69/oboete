# Owner decisions for the redesign (2026-09-24 evening)

Collected in a brainstorming interview; these override anything older (plan.md, search-sync proposal) where they conflict.

1. Redesign from a blank slate. The previous design is suspected of being shaped by the TypeScript prototype, not in one part but overall.
2. Extreme lightness is not a goal. Resident background processes are acceptable if they earn their place.
3. Goals, equal weight: resume where work stopped (across sessions, agents, devices); keep the owner's decisions, preferences and past failures and never apply retracted ones; look up the past with accurate, evidenced answers.
4. Curation is fully automatic. The owner never approves memories; the owner only corrects mistakes when noticing them.
5. AI usage is user-selectable in tiers (none / free + local / subscription CLIs with a daily cap / paid APIs with a monthly cap). With no AI, recording and search still work; more AI means finer curation. The owner's own default is the subscription tier with paid APIs at most USD 5 per month.
6. Devices: knowledge reaches other devices within a few minutes (Claude recommended, owner delegated). WSL and Windows native are the same PC and used side by side; the third device is an M1 iMac.
7. Raw work records (redacted) are kept forever, compressed, locally; individual items can be deleted.
8. Public release is a first-class goal from the start: easy install for anyone is as important as the owner's own use. (Separately from the older rule "finish every feature before release".)
9. Where an always-on server would live, and whether to archive raw records now, are postponed until the design is settled.
10. After the Phase 0 spike (phase0.md), approach B (self-built) is chosen. Before refining the design, find further improvements and strengthening points.

Unchanged earlier decisions that still hold: claude-mem is never stopped, deleted or reconfigured by us; the 7 agents (Claude Code, Codex, Grok Build, agy, OpenCode, Pi, Cursor CLI and IDE); Japanese and English; import claude-mem history read-only; Cloudflare Workers Paid is available, paid APIs capped; global preferences only from explicit owner statements; deletion reaches every device; per-repo sync exclusion; updates only by explicit `oboete update`; viewer on demand.

## 2026-09-25 (after the improvement sweep)

11. All 23 MUST items of improvements-synthesis.md go into the design (build roughly doubles; consistent with "finish every feature before release").
12. Folder transport is postponed: the first release syncs through the Cloudflare hub only; one device needs no cloud; the docs say multi-device needs the hub. Revisit after release on demand.
13. No code signing at first: install by command (curl | sh, PowerShell) where no Mark-of-the-Web/quarantine warning is expected (to be confirmed on real machines before release); sign later if needed.
14. Raw sync is chosen at setup (one question, off by default); an encrypted off-device backup of raw is decided later with decision 9.
15. Subscription CLIs wait while the owner is working (they share the owner's quota); free, local and paid providers do not wait. Default 10 minutes without a hook, a setting (decided by Claude, owner may overrule).
16. User settings added (decided by Claude, owner delegated): capture exclusion per repo or folder (not only sync exclusion); extra redaction rules and an allowlist for false positives (built-in rules cannot be removed); injection on/off and size per kind (SessionStart, per prompt, mid-session correction); raw retention period (default forever); capture detail (whether prompts are stored, tool output full or head+tail); backup location (the interval is set by measurement). Safety rules stay fixed: decision gates, the global-scope channel, data fencing, built-in redaction, deletion propagation.
17. Sections 1-4 are settled as in sections-1-4.md (2026-09-25).
18. Section 5 is settled as in section-5-revised.md (2026-09-25). The hub stays on Cloudflare (hub-platform.md). No Cloudflare Access anywhere: devices use hub-issued tokens, and the Claude app logs in through the Worker's own OAuth with a device-issued approval code (owner: "Access を使ってもあまり意味がないなら推奨で良い"). Zone mTLS on one of the owner's domains is optional hardening; the zone is chosen when the hub is built.
19. Section 6 is settled as in section-6-revised.md (2026-09-25): four levels (never record, mute, withdraw, forget), forget irreversible with a preview and no trash, no app-level encryption in the first release, curator tools checked on every call. PR #51 (merged 8f3ab0a) took agy out of the default chain now.
20. Raw deletion stopped now (decision 9's second half, 2026-09-25): PR #52 (merged 96cc106) keeps raw events after summarizing, with `sessions.observed_event_id` as the observe cursor.
21. Section 7 is settled as in section-7-revised.md (2026-09-25). Subscription CLIs in public setup: explicit opt-in, off by default, with the policy quoted (option b). Linux arm64 and macOS x64 ship as "CI-verified only". No scheduled update check: `oboete update --check`, doctor's last-check line and GitHub Security Advisories.
22. Section 8 is settled as in section-8-revised.md (2026-09-25). The owner's machines switch to the new oboete after milestone 4 (dogfood user first, old binary kept for rollback). The owner gives the full labelling set (about 13-19 h, in sittings of an hour or less, candidates drafted by Claude). All eight sections are settled; next is the written spec.
23. (2026-09-25, after the sections were settled) The grok subscription is no longer used for curation (nor judge or digest). To research and adopt if possible: how claude-mem uses the Claude subscription, and OpenCode's free models and OpenCode Go models as providers.

## 2026-09-26 (while the spec was compiled; docs/spec.md 0.1 is the running list)

24. Issue #54 stopgap in the current code (PR #57): long sessions go to the summarizer in parts, dialogue first. Design B's curation windows replace it.
25. OpenCode Go (the owner already subscribes) is a curator provider as an API-key subscription (`glm-5.3-flash`), marked subscription. OpenCode Zen free models are not offered (HTTP 403 for callers other than OpenCode). The opencode CLI is not a curator.
26. Curators on subscriptions use cheap models, as claude-mem does: claude Haiku 4.5, codex `gpt-6-luna` at low reasoning effort (PR #58 for today's code); the cheapest model that passes milestone 3's M3 lines is the default.
27. The owner's machines switch after milestone 5 (forget and safety), not after milestone 4 (replaces that part of decision 22).
28. Public setup turns subscriptions on by default (replaces decision 21's opt-in, option b); the tier line still quotes the policy, including line 52, and says how to turn it off.
