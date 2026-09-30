# Milestone 4: Deliver

The plan is docs/milestone-4-plan.md. This note keeps what the tasks measured.

## Task 3: claude-mem's history on Design B

`oboete --home ~/.oboete/eval/b-import import claude-mem ~/.oboete/eval/claude-mem-2026-09-24.db --eval-store`, a release build on WSL (ext4), then `oboete rebuild` (every consumer, no AI call).

- **Documents:** 178,370 import ops: 152,030 observations, 13,155 session summaries, 13,185 prompts; 2,100 rows had nothing left to store (empty, or a harness notification). Their source ids are exactly those of v1's evaluation store's `imports` table (178,370 each way, none apart), so the 112-question judgments map through it (D10). The plan's 178,502 counted that store's tables, not its imports.
- **Source name:** the import names the database `claude-mem:b62f07076e19` (its first migration time). v1's evaluation store was built before that naming and calls it `claude-mem`: a mapping from B's uids to its documents goes by source id.
- **Time:** the import took 40 s (46 MB peak memory); the rebuild 60 s (22 MB).
- **Size:** raw.db 323 MB; knowledge.db 864 MB, of which the trigram index is 531 MB and the documents 274 MB. The index keeps no copy of the text (`content=''`); with one, knowledge.db was 1,115 MB. The backup's ops segments are 58 MB.
