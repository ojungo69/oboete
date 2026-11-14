use crate::claims::{ClaimOp, Evidence};
use crate::raw::{Event, ImportDoc, OpKind, Raw};
use serde_json::json;

pub struct Store {
    pub home: tempfile::TempDir,
    pub raw: Raw,
}

impl Store {
    pub fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        Self { home, raw }
    }

    /// A prompt of `session` in `repo` at `ts`: its seq.
    pub fn said(&mut self, session: &str, repo: &str, ts: i64, text: &str) -> i64 {
        let e = Event {
            kind: "prompt".into(),
            session: session.into(),
            repo: Some(repo.into()),
            ts,
            ..crate::raw::test_event(&json!({ "prompt": text }).to_string())
        };
        self.raw.append(&e).unwrap()
    }

    /// An event of `kind` of `session` on checkout `(repo, branch)` at `ts`: its seq.
    pub fn event(
        &mut self,
        kind: &str,
        session: &str,
        (repo, branch): (&str, &str),
        ts: i64,
        body: serde_json::Value,
    ) -> i64 {
        let e = Event {
            kind: kind.into(),
            session: session.into(),
            repo: Some(repo.into()),
            branch: Some(branch.into()),
            ts,
            ..crate::raw::test_event(&body.to_string())
        };
        self.raw.append(&e).unwrap()
    }

    /// A claim that quotes record `seq` whole (`text`, the record's), and its uid.
    pub fn claim(
        &mut self,
        seq: i64,
        text: &str,
        (kind, status, speaker): (&str, &str, &str),
        supersedes: &[&str],
    ) -> String {
        let evidence = Evidence {
            device: self.raw.device().to_owned(),
            seq,
            offset: 0,
            length: text.len() as i64,
            sentence: 0,
            quote: text.into(),
            claim_at: None,
        };
        let uid = crate::claims::uid(kind, &evidence);
        let op = ClaimOp {
            id: format!("c{seq}"),
            kind: kind.into(),
            status: status.into(),
            speaker: speaker.into(),
            scope: "repo".into(),
            body: text.into(),
            evidence: vec![evidence],
            supersedes: supersedes.iter().map(|s| s.to_string()).collect(),
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
            support: Vec::new(),
        };
        let op = serde_json::to_value(op).unwrap();
        self.raw.append_ops(&[(OpKind::Claim, op)]).unwrap();
        uid
    }

    /// A record of `session` in `repo` at `ts` and the user's decided decision quoting it.
    pub fn decided(&mut self, repo: &str, ts: i64, text: &str, supersedes: &[&str]) -> String {
        let seq = self.said("s", repo, ts, text);
        self.claim(seq, text, ("decision", "decided", "user"), supersedes)
    }

    /// A claude-mem document of `project` at `ts`: its uid.
    pub fn imported(
        &mut self,
        id: &str,
        project: &str,
        ts: i64,
        title: &str,
        body: &str,
    ) -> String {
        let doc = ImportDoc {
            uid: format!("claude-mem:test:{id}"),
            source: "claude-mem:test".into(),
            source_id: id.into(),
            kind: "decision".into(),
            repo: crate::import::repo(project),
            session: "cm".into(),
            ts,
            title: title.into(),
            body: body.into(),
        };
        let uid = doc.uid.clone();
        self.raw.append_imports(vec![doc]).unwrap();
        uid
    }

    /// A note an agent saved (`save_memory`) to `repo` at `ts`: its uid.
    pub fn note(&mut self, id: &str, repo: &str, ts: i64, title: &str, body: &str) -> String {
        let doc = ImportDoc {
            uid: format!("{}:{id}", crate::raw::NOTE_SOURCE),
            source: crate::raw::NOTE_SOURCE.into(),
            source_id: id.into(),
            kind: "note".into(),
            repo: repo.into(),
            session: crate::raw::NOTE_SESSION.into(),
            ts,
            title: title.into(),
            body: body.into(),
        };
        let uid = doc.uid.clone();
        self.raw.append_imports(vec![doc]).unwrap();
        uid
    }

    /// An imported observation `id` of `kind` in `session`, filed under claude-mem project
    /// `project`, with a files op naming `read` and `modified` (docs/file-note.md F10): its uid.
    #[allow(clippy::too_many_arguments)]
    pub fn imported_with_files(
        &mut self,
        id: &str,
        (project, session): (&str, &str),
        kind: &str,
        ts: i64,
        title: &str,
        read: &[&str],
        modified: &[&str],
    ) -> String {
        let uid = format!("claude-mem:test:{id}");
        let doc = ImportDoc {
            uid: uid.clone(),
            source: "claude-mem:test".into(),
            source_id: id.into(),
            kind: kind.into(),
            repo: crate::import::repo(project),
            session: session.into(),
            ts,
            title: title.into(),
            body: format!("{title}."),
        };
        self.raw
            .append_import_items(
                vec![
                    crate::raw::Import::Doc(doc),
                    crate::raw::Import::Files(self.files_of(
                        id,
                        (project, session),
                        read,
                        modified,
                    )),
                ],
                |_| {},
            )
            .unwrap();
        uid
    }

    /// A files op for imported document `id` (F10), as `oboete import claude-mem` appends one.
    pub fn files_of(
        &self,
        id: &str,
        (project, session): (&str, &str),
        read: &[&str],
        modified: &[&str],
    ) -> crate::raw::ImportFiles {
        let list = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
        crate::raw::ImportFiles {
            uid: format!("claude-mem:test:{id}"),
            source: "claude-mem:test".into(),
            source_id: id.into(),
            repo: crate::import::repo(project),
            session: session.into(),
            files_read: list(read),
            files_modified: list(modified),
        }
    }

    /// claude-mem documents `(id, session, kind, ts, body)` of one project, appended at once:
    /// their uids.
    pub fn imported_all(&mut self, docs: Vec<(String, &str, &str, i64, String)>) -> Vec<String> {
        let docs: Vec<ImportDoc> = docs
            .into_iter()
            .map(|(id, session, kind, ts, body)| ImportDoc {
                uid: format!("claude-mem:test:{id}"),
                source: "claude-mem:test".into(),
                source_id: id,
                kind: kind.into(),
                repo: crate::import::repo("p"),
                session: session.into(),
                ts,
                title: String::new(),
                body,
            })
            .collect();
        let uids = docs.iter().map(|d| d.uid.clone()).collect();
        self.raw.append_imports(docs).unwrap();
        uids
    }

    /// The owner's correction of claim `uid` (`oboete correct`'s op); `run` applies it.
    pub fn correct(&mut self, uid: &str, status: Option<&str>, body: Option<&str>) {
        let k = crate::knowledge::open(self.home.path()).unwrap();
        let c = crate::claims::active_one(&k, uid).unwrap().unwrap();
        let op = crate::claims::CorrectionOp {
            uid: uid.into(),
            anchor: crate::claims::Anchor {
                device: c.device,
                seq: c.seq,
            },
            status: status.map(Into::into),
            body: body.map(Into::into),
            muted: None,
        };
        let op = serde_json::to_value(op).unwrap();
        self.raw.append_ops(&[(OpKind::Correction, op)]).unwrap();
    }

    pub fn exclude(&mut self, repo: &str) {
        let op = json!({"repo": repo, "undo": false});
        self.raw.append_ops(&[(OpKind::Exclusion, op)]).unwrap();
    }

    /// The consumers' pass, which search reads.
    pub fn run(&self) {
        crate::worker::run_once(self.home.path()).unwrap();
    }

    pub fn query(&self, q: &super::Query) -> super::Answer {
        super::query(self.home.path(), q).unwrap()
    }

    pub fn key(&self, seq: i64) -> String {
        format!("{}:{seq}", self.raw.device())
    }

    /// Invented window observations, written through the same op log as curation.
    pub fn cards(
        &mut self,
        from: i64,
        to: i64,
        observations: serde_json::Value,
        recurate: bool,
    ) -> Vec<String> {
        let count = observations.as_array().unwrap().len();
        let op = json!({"outcome": "curated", "summary": "", "from_seq": from,
                "to_seq": to, "from_offset": null, "to_offset": null, "elided": [],
                "removed": [], "goals": [], "observations": observations, "recurate": recurate});
        let seq = self.raw.append_ops(&[(OpKind::Window, op)]).unwrap()[0];
        (0..count).map(|n| format!("{seq}.{n}")).collect()
    }

    pub fn turn(&mut self, op: serde_json::Value) -> String {
        let seq = self.raw.append_ops(&[(OpKind::Turn, op)]).unwrap()[0];
        format!("S{seq}")
    }
}
