# oboete

Memory for coding agents: one program records each agent's sessions on the user's own machine,
makes reusable knowledge of them, and hands what matters to the next session of any agent. The page
shows each term in Japanese and English alike (decision 114); the Japanese is in brackets.

## Language

### What is recorded

**Record** (記録):
One event as it was stored: a prompt, a reply, a tool call or its result, or an imported memory.
_Avoid_: log, entry, observation

**Conversation** (会話):
One session of one agent, from its start to its end. In English on the page: session.
_Avoid_: セッション on the Japanese page, thread, chat

**Repository** (リポジトリ):
Where work happens, known by its origin, so that its worktrees are one repository.
_Avoid_: project, プロジェクト, folder

### What is made from it

**Claim** (覚えたこと):
One fact kept from the work, such as a decision, a preference or a lesson, with the lines that show it.
In English on the page: a memory, and their list is Memories.
_Avoid_: 主張 on the page, 記憶 for a single claim, Learned, observation

**Card** (カード):
What one stretch of work was, in the shape of claude-mem's observation: a title, a subtitle, and what
was done and found.
_Avoid_: observation until the page's English is settled; カード表示 is My Work's view, not a card

**Conversation summary** (会話の要約):
What a conversation investigated, learned and completed, and what comes next, written when it stops.
Its sections keep claude-mem's names: 学んだこと is Learned. In English: session summary.
_Avoid_: セッションの要約

**Agent memory** (エージェントに渡す記憶):
What oboete hands an agent's session: at its start, at a prompt and at a tool call.
_Avoid_: context or injection on the page

### My Work

**Request** (依頼):
One piece of work the owner asked for, across the conversations that carry it, with its state.
_Avoid_: task, ticket, card

**Focus area** (分野):
A theme the owner's requests come under, with its goal.
_Avoid_: focus on its own, project, category

**Rule** (規則):
A sentence every agent's session is given, for every repository or for one.
_Avoid_: preference (a kind of claim), directive, instruction

**Recent activity** (最近の動き):
A request's latest records, as the request's detail lists them. In English: Latest records.
_Avoid_: 記録, which is the Records tab

**Done words**:
完了 is a request that shipped (Shipped). 達成 is a focus area the owner finished (Done). 作業を終えた
is a subagent that finished its work (Finished). 閉じた is a request closed without shipping (Closed).
_Avoid_: 完了 for anything but a shipped request, 終了

### What the owner does

**Forget** (忘れる):
Removing records and everything made from them, for good, after a preview of what goes.
_Avoid_: delete; purge, which is the step that rewrites the store

**Mute** (ミュート):
Keeping one claim out of agent memory, while search still finds it.
_Avoid_: hide, turn off

**AI** (AI):
A service oboete calls to summarize, to judge or to embed. Where it is the one that summarizes: 要約に使う
AI.
_Avoid_: 接続先 and 要約役 on the page; provider is the code's word

### The store

**Pause**:
A stretch with no hook record, long enough that the worker's builds of several seconds are unlikely to
make a hook wait.
_Avoid_: idle time, quiet period, downtime

**Optional index**:
An index the store of records can be without, made after the store exists by a builder.
_Avoid_: lazy index, worker index

**Builder**:
One of the fixed occasions allowed to make optional indexes, such as the pause.
_Avoid_: maker, payer, need
