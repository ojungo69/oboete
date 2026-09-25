# oboete hub platform: architect's answer (2026-09-25)

Legend: `+` meets, `~` partial or conditional, `-` fails, `?` unmeasured. R1 to R9 are the requirements in the brief. Claims come from the verified cards. Where a verifier refuted a claim, the corrected text is used. Facts I checked myself this session are marked (checked).

## 1. Ranked comparison

| # | Platform | R1 op log | R2 purge | R3 device auth | R4 MCP, vectors, FTS | R5 cost | R6 setup | R7 latency | R8 own account | R9 lock-in | Decisive reason for the rank |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | **Cloudflare**: Worker + one named SQLite DO (+ R2, + Vectorize optional) | + one DO is the single writer | ~ rows, FTS, vectors and R2 objects purge; the 30-day DO PITR cannot be purged (bounded, disclosed) | ~ hub tokens, revoked one by one; pre-app gate only with the user's own zone (mTLS) | + workers-oauth-provider; FTS5 trigram inside the DO; Vectorize needs Paid | + owner about +USD 0.5/mo; public USD 0 for sync and FTS-only MCP | ~ wrangler needs Node; direct API upload unproven | ? DO placement unmeasured | + | ~ protocol portable, implementation CF-specific | The only option that is free and card-less for public sync **and** near-free for the owner, with a ready OAuth provider for the MCP |
| 2 | **Self-hosted Rust hub** (`oboete hub serve` on Fly nrt, Railway or a VPS) | + SQLite WAL, one process | + snapshots are operator-set (Fly 1 to 60 days) or off | ~ same app tokens; no edge gate | ~ reuses oboete's FTS5 and sqlite-vec bit index; OAuth server is DIY | ~ owner USD 3 to 5/mo (Fly price not confirmed; Railway flat 5) | - public users must provision a VM; card needed | + Fly nrt (Tokyo); Railway region unverified | + | + same binary on any host | Wins on purge, portability and search reuse, but no free card-less host exists and the OAuth server is a security-sensitive build |
| 3 | **AWS**: API Gateway + Lambda + DynamoDB + S3 Vectors | ~ ordering counter is hand-built | + PITR opt-in, off by default | + authorizer blocks before Lambda | ~ S3 Vectors in Tokyo; Japanese FTS needs a separate search service | ~ Always Free tier, but a second bill | - payment method at signup, IAM/CloudFormation | + Tokyo | + | ~ proprietary data models | Best pre-application auth, but no cheap Japanese FTS next to a single writer, and the heaviest setup |
| 4 | **Deno Deploy** + Deno KV | ~ atomic CAS, DIY log | ? no PITR found | - no pre-app gate | - no vectors or FTS | + free, no card; KV 1 GiB | + single binary | ~ edge; KV region unknown | + | ~ Deno-specific KV | Best card-less, Node-free public setup, but 1 GiB of KV and no search push the MCP onto extra services |
| 5 | **Supabase** | + Postgres, bigserial cursor | ~ 7-day backups, no per-row purge | ~ revocation is an app-level check | + pgvector, pgroonga; MCP/OAuth DIY | - Pro USD 25/mo; Free pauses after 7 idle days | ~ binary CLI | + Tokyo | + | + open-source stack | The USD 25/mo floor is five times the owner's extra budget, and Free pauses itself |
| 6 | **Turso** | - row-level last-push-wins sync, not an op log | + PITR bounded by plan (1 to 90 days) | - only "invalidate all tokens" | - no compute; its MCP manages Turso itself | + free, no card | ~ binary CLI, but still needs a compute host | + Tokyo | + | ~ sync protocol not independently documented | Not a hub: a database that still needs a Worker or VM in front, and it cannot revoke one device |
| 7 | Firebase / Convex / Fastly | ~ | ~ | - | - Firebase MCP needs Blaze; Convex has no Asia region; Fastly KV is not a log | ~ | - Firebase and Convex CLIs need Node | ~ | + | - Convex most locked-in | Each fails one hard requirement |
| 8 | Sync engines (PowerSync, Electric, cr-sqlite, Automerge, sqlite-sync) | - multi-writer merge, no ack layer | - CRDT tombstones must stay; no purge path | - | - | - PowerSync/Electric need Postgres + Docker | - | ~ | + | ~ | They solve a multi-writer merge problem oboete does not have, and ship none of transport, auth, purge or MCP |

Corrected claims used above:
- **DO Free write cap (100k rows/day):** the first push of about 1M rows is a scheduling choice, not a wall. It can be spread over about 10 days, or run during one month on Paid (50M writes/month, no daily cap).
- **Turso:** the claim that one DELETE purges vectors and FTS together was refuted for lack of a source. It is plausible for native columns, but unsourced, so it does not count for R2.
- **sqlite-sync:** the license is ELv2 with an open-source carve-out, and its transport is plain, pluggable HTTP. The rank does not change, because the reason is the CRDT model, not the license.
- **Card requirements:** OCI Always Free signup needs a card, except for special groups (Oracle Academy and similar). AWS "might request" payment information, and its FAQ requires a valid payment method for the free plan.

**Does anything beat Cloudflare?** Not on the whole set. Each alternative beats it on one axis:
- self-host on purge and portability;
- AWS on pre-application auth;
- Supabase on search quality (pgroonga);
- Deno on card-less, Node-free setup.

None of them does all of this inside USD 5/month with a free path for public users.

**OCI Always Free** is left out on purpose, although the self-host card named it best for the owner:
- The owner's existing A1 box runs p-cipher production (a Monero hot wallet), and it must not be stopped or resized (memory `oci-a1-instance-constraints`). An internet-facing hub does not belong next to a hot wallet.
- A new Always Free box is now 2 OCPU / 12 GB. Oracle takes it back when its 7-day p95 CPU, network and memory are all under 20%. A hub serving a few thousand requests a day would stay under that line.

## 2. Recommendation

**Owner and public users use the same Cloudflare hub:** one Worker and one fixed-name SQLite DO, plus R2 and Vectorize only when chosen, as section 5 already plans. One implementation means one thing to test and one set of purge paths to audit.

- **Owner:** already on Workers Paid.
  - Vectorize costs cents: about USD 0.09/mo stored and under USD 0.50/mo in queries at 3,000 queries/day, per the card's arithmetic. That is under the USD 1.5 stop line.
  - The 30-day DO history is the one retention the owner cannot purge. It is bounded and documented.
- **Public users:** a Cloudflare account needs only an email and a password, and "By default, users have access to the Workers Free plan" (developers.cloudflare.com/fundamentals/account/create-account/, developers.cloudflare.com/workers/platform/pricing/; checked).
  - Free covers sync and a remote MCP with full-text search only.
  - Remote semantic search needs Paid (USD 5/mo), unless spike item 4 removes Vectorize.
- **Remote MCP login:** drop Cloudflare Access from section 5. This reverses proposal:364, which section 5 treated as settled. It is Claude's decision; the owner may overrule it.
  - workers-oauth-provider has modes that need no Zero Trust (verified card claim).
  - Proposed default: the Worker handles OAuth itself. The login page asks for a short-lived, single-use approval code that an already-enrolled device creates (`oboete hub approve`).
  - The alternative is GitHub OAuth, restricted to one user id.
  - This removes the last Zero Trust dependency, and its payment-details step, from the whole hub. It is a security path, so it gets the rules/security.md review.
- **Protocol:** write the wire contract down as its own document, `docs/hub-protocol.md`. It covers:
  - the endpoints, the op envelope and its format version, and the cursor;
  - per-op ack by (id, hash), tombstone precedence and the two-pass pull that serves control ops first;
  - the error codes (413 and others) and the auth header.
- **Both hubs can coexist, but only one gets built now.**
  - The client's own tests need a fake hub anyway. If that fake follows the protocol document, it becomes the reference other hubs are checked against, and a later Rust `oboete hub serve` needs no client changes.
  - Building the second hub now is not justified: nothing in R1 to R9 requires it, and it would double the OAuth and purge surface.

**What would change the recommendation**
1. The Node-free deploy fails (spike item 1) and no Deploy-button route works. Public setup then needs Node, which breaks R6 for Cloudflare. Deno Deploy or a Rust self-host binary would then serve public users.
2. Measured remote-MCP p95 stays above 1 s even with a location hint. Move the DO, or accept a self-hosted hub in Tokyo.
3. The owner needs a purge bound shorter than 30 days, for example for legal reasons. Use the self-hosted Rust hub with snapshots short or off, or AWS with PITR off. The documented protocol allows both.
4. Cloudflare reprices the Free limits, or Vectorize crosses USD 1.5/month.
5. The in-DO bit search (spike item 4) passes. Cloudflare stays, but Vectorize leaves the design and semantic remote search works on Free.

## 3. Device auth

**Is Cloudflare Access worth making the default? No, for either audience.**
- **Public users:** Zero Trust onboarding asks for payment details even on Free (developers.cloudflare.com/cloudflare-one/setup/, step 3). That contradicts decision 8 on the basic multi-device path.
- **Access mTLS:** service-auth mTLS is "available with Enterprise and pay-as-you-go Zero Trust plans. It is not included in the Free plan" (developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/mutual-tls-authentication/; checked).
- **Owner:** the owner's card is already on file, so the payment step is a one-time click, and Access service tokens would give a per-device gate before application code.
  - But hub tokens already meet the core of R3: each device is revoked on its own, and no token is an account key.
  - On Workers Paid, a flood of rejected requests costs cents instead of stopping sync.
  - Access would add a second product, and its service tokens expire and need renewal.

**Defaults**
- **Public users:** hub-issued bearer tokens, one per device, on `workers.dev` with a random path secret. The hub stores only the SHA-256, and deleting a row revokes one device.
  - This does **not** meet R3's "before application code". On `workers.dev` no free pre-application gate exists. The token check is the first line inside the Worker, before any DO call.
  - Zone mTLS (below) is the upgrade for users who bring their own domain.
  - The docs state the Free trade-off: a leaked URL can use up the daily request quota until the reset, and the data stays safe.
- **Owner:** the same hub tokens. The owner has four zones on Cloudflare (checked 2026-09-25), so the free zone-mTLS gate below is available as hardening.
  - Serve sync on its own hostname (for example `sync.<domain>`) with zone mTLS and a WAF block rule, and turn off `workers.dev`.
  - Keep the MCP on a separate hostname without mTLS, because the Claude app cannot present a client certificate.
  - The change is config plus a small client change: `oboete hub device add` requests a Cloudflare-managed client certificate through the API, and reqwest presents it.
  - Ship it after the plain token path works.

**Free pre-application gates on Cloudflare** (all checked this session; all need the user's own zone)

| Gate | Free? | Revocable per device? | Source |
|---|---|---|---|
| Zone mTLS with the Cloudflare-managed CA, enforced by a WAF rule (`not cf.tls_client_auth.cert_verified`, plus the revoked-certificate check in /api-shield/security/mtls/configure/#check-for-revoked-certificates) | Yes: "Anyone can set up Mutual TLS with a Cloudflare-managed certificate authority" | Yes: 100 active certificates per zone; revoking frees the slot; the rule must also check for revoked certificates | developers.cloudflare.com/api-shield/ (Availability); /ssl/client-certificates/; /ssl/client-certificates/revoke-client-certificate/; rule expression in /ssl/client-certificates/byo-ca/ |
| WAF custom rules (for example, block when a header secret is missing) | Yes: 5 rules, no regex | Only by editing the rule; the secrets sit in the config in plain text | developers.cloudflare.com/waf/custom-rules/ |
| Rate limiting rules | Yes: 1 rule, 10 s mitigation timeout | No (throttle only) | developers.cloudflare.com/waf/rate-limiting-rules/ |
| IP Access rules | Yes: 50,000 | No use for home and mobile IPs | developers.cloudflare.com/waf/tools/ip-access-rules/ |
| API Shield JWT or schema validation | No: "API Security products are available to Enterprise customers only" | n/a | developers.cloudflare.com/api-shield/ |
| Access service tokens | Zero Trust Free, with the payment-details step | Yes | developers.cloudflare.com/cloudflare-one/setup/ |
| Access mTLS (service auth) | No (Enterprise or pay-as-you-go) | n/a | the Access mTLS page cited above |

Caveats:
- **Zone needed:** every gate in the table needs a hostname in a Cloudflare zone the user owns. The Client Certificates and WAF pages are per zone, and `workers.dev` is not the user's zone. This is my inference; I found no sentence that says it for workers.dev.
- **Quota:** that WAF-blocked requests do not count as Worker requests is also an inference. The pricing page bills "inbound requests to your Worker", and the WAF runs before the Worker. Spike item 6 checks it.
- **Outside Cloudflare:** AWS API Gateway authorizers are the only other pre-application gate in the cards, and they come with an account that needs a card. Tailscale ACLs help only the owner's own tailnet, and the Claude app cannot join it.

## 4. Unknowns and the spike that settles them

One throwaway Worker plus DO, about 1 to 2 days, run on a **fresh Free account** and on the owner's Paid account. Each item has a pass line. Items 1 to 3 decide the platform question; items 4 to 8 are hardening and optimization, and item 4 is optional.

1. **Node-free deploy.**
   - The Script Upload API accepts `migrations` in the multipart metadata for uploads that deploy immediately (developers.cloudflare.com/workers/configuration/multipart-upload-metadata/; checked).
   - But the IaC page warns that a DO binding plus its migration can fail on the first apply through the API and then needs two steps (developers.cloudflare.com/workers/platform/infrastructure-as-code/; checked).
   - The Versions API is ruled out: a version upload cannot create, delete, rename or transfer a DO class (developers.cloudflare.com/workers/versions-and-deployments/deployment-management/; checked).
   - Test: from a Rust binary, call `PUT /accounts/{id}/workers/scripts/{name}` (the upload that deploys immediately) with the bundled Worker, the DO binding and `new_sqlite_classes`. Then create the R2 bucket by REST. Try it as one call, then as two.
   - Also time how long a new user takes to create the scoped API token, since wrangler's OAuth login cannot be reused.
   - Pass: a working hub from one command on Windows, macOS and Linux, with no Node installed.
   - Fallback to check: a "Deploy to Cloudflare" button. I have not verified that it supports DO classes.
2. **Latency.**
   - A DO is created near the first `get()`, or where `locationHint` points (`apac` / `apac-ne`, best effort; developers.cloudflare.com/durable-objects/reference/data-location/; checked).
   - Checked 2026-09-25: Claude app connectors call from Anthropic's cloud (egress 160.79.104.0/21), not from the user's machine (support.claude.com/en/articles/11175166; platform.claude.com/docs/en/api/ip-addresses). The region is not published.
   - Measure p50/p95 of an MCP search from a US host and of a push/pull from Japan, once with the DO hinted `apac-ne` and once unhinted.
   - Pass: MCP p95 at or below 1 s. Sync needs only minutes.
3. **Japanese full-text search in the DO.**
   - Run `CREATE VIRTUAL TABLE t USING fts5(x, tokenize='trigram')` in the DO, then query it.
   - Also test 2-character Japanese queries (for example 設計). Trigram cannot match fewer than 3 characters, so the fallback (a LIKE scan or a bigram column) must be chosen here, not later.
   - Pass: recall for queries of 3 or more characters matches the local store, and a 2-character query returns something useful.
4. **Semantic search in the DO without Vectorize.**
   - oboete already searches locally by sign-bit Hamming distance plus fp32 rescoring (src/embed.rs, PR-D1).
     - For 180k × 1024 bits that is about 23 MB of bits, which fits in DO memory. The fp32 rows (about 737 MB) fit in the 5 GB limit.
     - sqlite-vec is not in workerd, so the scan would be a hand-written Wasm popcount loop.
   - On Free, the likely failure is rows read, not CPU.
     - The local path rescores 4k candidates, which is 4k rows read per query. At the card's 3,000 queries/day that is 12M rows/day, against the 5M/day Free cap.
     - So store the bits as a handful of blob rows, not 180k rows. Check the DO SQLite per-row size limit first; I did not find it this session.
   - Measure: rows read per query, recall with about 1k rescored candidates compared with the local 4k, and CPU time for the Hamming scan.
   - Pass: recall close to the local result, rows read × expected queries under 5M/day, and p95 at or below 300 ms. Then Vectorize is dropped: semantic remote search works on Free, and there is one fewer store to purge.
5. **mTLS hardening.**
   - Test issuing a Cloudflare-managed client certificate through the API, enforcing it only on the sync hostname, and reqwest presenting it on all three OSes. Also measure how fast a revoke takes effect.
   - Pass: a revoked device gets a 403 before the Worker runs, and the MCP hostname is unaffected.
6. **Quota under a flood.**
   - Send 10k rejected requests on Free: (a) with a bad token, rejected by the Worker; (b) blocked by a WAF rule.
   - Check which of the two count against the 100k requests/day.
7. **First push on Free.**
   - Count the rows written for the owner's real history.
   - Pass: the throttled push resumes across days without loss, or the docs tell the user to run one month on Paid.
8. **OAuth end to end.**
   - Connect a Claude custom connector to workers-oauth-provider in self-handled mode, using the approval code created by a device.
   - Pass: it connects, per-repo grants hold, and results stop coming back at return time once a grant is revoked.
   - Also pass: the approval code has enough entropy, expires within minutes, works once, and the login endpoint is rate limited.

Items 1, 2 and 4 decide the platform question. Items 3 and 5 to 8 decide details within Cloudflare.