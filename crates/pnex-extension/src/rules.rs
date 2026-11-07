//! Player of navigation rules in the user's browser (agent-unification.md
//! D781–D783). It plays the plan the server sealed, to its end, in the tab
//! of the rule's platform, at a reader's pace (D258), and sends back each
//! step, each extract and the outcome. What a step does inside the page is
//! `rule_page.js`, the same code the server browser runs; only the order of
//! the steps is written here. The server checks every page again on return.

use pnex_core::browser_agent::AgentMsg;
use pnex_core::site_recipe::{
    between, contains_loose, fill, reason, site_host, AssertSpec, ExtractKind, ExtractSpec,
    ForEachSpec, RulePlan, RunExtract, RunStep, Step, StepStatus, Target, TypeSpec, WaitSpec,
    CARDS_MAX, FOR_EACH_MAX,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::host::{self, text};

/// Time given to a late element (banners, rendering after load), like the
/// server browser.
const FIND_GRACE_MS: f64 = 3_000.0;
/// Pause after an action that may navigate.
const SETTLE_MS: f64 = 1_500.0;
/// Text kept per extract.
const EXTRACT_MAX: usize = 200_000;

type Fail = (&'static str, Option<String>);

/// One run being played.
pub(crate) struct Player<'a> {
    plan: &'a RulePlan,
    /// The platform's tab, opened at the first `open`.
    pub(crate) tab: f64,
    send: &'a dyn Fn(&AgentMsg),
    extracts: Vec<RunExtract>,
}

impl<'a> Player<'a> {
    pub(crate) fn new(plan: &'a RulePlan, tab: f64, send: &'a dyn Fn(&AgentMsg)) -> Self {
        Self {
            plan,
            tab,
            send,
            extracts: Vec::new(),
        }
    }

    fn fill(&self, s: &str) -> String {
        fill(s, &self.plan.params)
    }

    /// Plays every step and reports the outcome.
    pub(crate) async fn play(&mut self) {
        let steps = self.plan.recipe.steps.clone();
        for (i, step) in steps.iter().enumerate() {
            let started = host::now_ms();
            let outcome = self.step(i, step).await;
            if !matches!(step, Step::ForEach(_)) {
                self.report(i, step, &outcome, started, None).await;
            }
            if let Err((why, _)) = outcome {
                self.done(false, Some(i as u32), Some(why));
                return;
            }
            // A reader's pace between two steps (D258).
            host::sleep(800.0 + js_sys::Math::random() * 1_200.0).await;
        }
        self.done(true, None, None);
    }

    fn done(&self, ok: bool, failed_step: Option<u32>, why: Option<&str>) {
        (self.send)(&AgentMsg::RunDone {
            run_id: self.plan.run_id.clone(),
            ok,
            failed_step,
            reason: why.map(str::to_string),
        });
    }

    async fn report(
        &self,
        i: usize,
        step: &Step,
        outcome: &Result<bool, Fail>,
        started: f64,
        prefix: Option<usize>,
    ) {
        let (status, why, detail) = match outcome {
            Ok(true) => (StepStatus::Skipped, Some(reason::TARGET_NOT_FOUND), None),
            Ok(false) => (StepStatus::Ok, None, None),
            Err((w, d)) => (StepStatus::Failed, Some(*w), d.clone()),
        };
        let label = self.fill(&step.label());
        let step = RunStep {
            index: i as u32,
            action: step.action().into(),
            label: match prefix {
                Some(n) => format!("{n} · {label}"),
                None => label,
            },
            status,
            reason: why.map(str::to_string),
            detail,
            url: self.url().await,
            ms: (host::now_ms() - started).max(0.0) as u64,
            screenshot: None,
        };
        (self.send)(&AgentMsg::RunStep {
            run_id: self.plan.run_id.clone(),
            step,
        });
    }

    async fn page(&self) -> Value {
        if self.tab < 0.0 {
            return Value::Null;
        }
        host::read_page(self.tab)
            .await
            .ok()
            .and_then(|v| serde_json::from_str(&text(&v)).ok())
            .unwrap_or(Value::Null)
    }

    async fn url(&self) -> String {
        self.page().await["url"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    async fn text(&self) -> String {
        self.page().await["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, name: &str, args: Value) -> Value {
        if self.tab < 0.0 {
            return Value::Null;
        }
        let args = Value::Array(match args {
            Value::Null => vec![],
            a => vec![a],
        });
        host::rule_call(self.tab, name, &args.to_string())
            .await
            .ok()
            .and_then(|v| serde_json::from_str(&text(&v)).ok())
            .unwrap_or(Value::Null)
    }

    fn on_site(&self, url: &str) -> bool {
        site_host(url).as_deref() == Some(self.plan.site.as_str())
    }

    /// `Ok(true)`: skipped (an optional target missing).
    async fn step(&mut self, i: usize, step: &Step) -> Result<bool, Fail> {
        match step {
            Step::Open(path) => self.open(path).await,
            Step::Click(t) => self.click(t).await,
            Step::Scroll(to) => self.scroll(to).await,
            Step::Wait(w) => self.wait(w).await,
            // PDFs are fetched and read by the server browser only.
            Step::FollowPdf(p) if p.optional => Ok(true),
            Step::FollowPdf(_) => Err((reason::BROWSER, Some("follow_pdf".into()))),
            Step::Extract(e) => self.extract(i, e).await,
            Step::Assert(a) => self.assert(a).await,
            Step::Type(t) => self.type_text(t).await,
            Step::Press(k) => self.press(k).await,
            Step::ForEach(f) => Box::pin(self.for_each(i, f)).await,
        }
    }

    async fn open(&mut self, path: &str) -> Result<bool, Fail> {
        let path = self.fill(path);
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path
        } else {
            format!(
                "{}/{}",
                self.plan.base.trim_end_matches('/'),
                path.trim_start_matches('/')
            )
        };
        if !self.on_site(&url) {
            return Err((reason::OFF_SITE, Some(url)));
        }
        self.tab = host::agent_tab(self.tab, &url)
            .await
            .ok()
            .and_then(|v| v.as_f64())
            .ok_or((reason::BROWSER, Some("tab".into())))?;
        self.still_on_site().await
    }

    async fn still_on_site(&self) -> Result<bool, Fail> {
        let now = self.url().await;
        if now.is_empty() || now.starts_with("about:") || self.on_site(&now) {
            Ok(false)
        } else {
            Err((reason::OFF_SITE, Some(now)))
        }
    }

    fn target_spec(&self, t: &Target) -> Value {
        let f = |o: &Option<String>| o.as_deref().map(|s| self.fill(s));
        json!({
            "text": f(&t.text),
            "text_contains": f(&t.text_contains),
            "selector": f(&t.selector),
            "role": f(&t.role),
            "name": f(&t.name),
        })
    }

    /// Finds a target, giving late elements up to [`FIND_GRACE_MS`].
    async fn find(&self, spec: Value) -> (u64, Option<String>) {
        let until = host::now_ms() + FIND_GRACE_MS;
        loop {
            let out = self.call("pnexFind", spec.clone()).await;
            let found = out["found"].as_u64().unwrap_or(0);
            if found > 0 || host::now_ms() >= until {
                return (found, out["href"].as_str().map(str::to_string));
            }
            host::sleep(250.0).await;
        }
    }

    async fn click(&mut self, t: &Target) -> Result<bool, Fail> {
        if self.find(self.target_spec(t)).await.0 == 0 {
            return if t.optional {
                Ok(true)
            } else {
                Err((reason::TARGET_NOT_FOUND, None))
            };
        }
        self.call("pnexClick", Value::Null).await;
        let _ = host::settle(self.tab, SETTLE_MS).await;
        self.still_on_site().await
    }

    async fn type_text(&mut self, t: &TypeSpec) -> Result<bool, Fail> {
        let f = |o: &Option<String>| o.as_deref().map(|s| self.fill(s));
        let spec =
            json!({ "text": self.fill(&t.text), "selector": f(&t.selector), "role": f(&t.role) });
        let out = self.call("pnexType", spec).await;
        if out["ok"].as_bool() == Some(true) {
            return Ok(false);
        }
        match out["reason"].as_str() {
            Some(reason::NOT_A_SEARCH_FIELD) => Err((reason::NOT_A_SEARCH_FIELD, None)),
            _ if t.optional => Ok(true),
            _ => Err((reason::TARGET_NOT_FOUND, None)),
        }
    }

    async fn press(&mut self, key: &str) -> Result<bool, Fail> {
        self.call("pnexPress", json!(key)).await;
        let _ = host::settle(self.tab, SETTLE_MS).await;
        self.still_on_site().await
    }

    async fn scroll(&mut self, to: &str) -> Result<bool, Fail> {
        let to = self.fill(to);
        if to.trim() == "bottom" {
            for _ in 0..3 {
                let _ = host::scroll(self.tab).await;
                host::sleep(400.0).await;
            }
        } else if self.find(json!({ "selector": to })).await.0 == 0 {
            return Err((reason::TARGET_NOT_FOUND, None));
        }
        host::sleep(500.0).await;
        Ok(false)
    }

    async fn wait(&mut self, w: &WaitSpec) -> Result<bool, Fail> {
        if let Some(ms) = w.ms {
            host::sleep(ms as f64).await;
            return Ok(false);
        }
        let want = self.fill(w.text.as_deref().unwrap_or_default());
        let until = host::now_ms() + w.timeout_ms.unwrap_or(10_000) as f64;
        while host::now_ms() < until {
            if contains_loose(&self.text().await, &want) {
                return Ok(false);
            }
            host::sleep(300.0).await;
        }
        if w.optional {
            Ok(true)
        } else {
            Err((reason::TIMEOUT, None))
        }
    }

    fn push(&mut self, e: RunExtract) {
        (self.send)(&AgentMsg::RunExtract {
            run_id: self.plan.run_id.clone(),
            extract: e.clone(),
        });
        self.extracts.retain(|o| o.name != e.name);
        self.extracts.push(e);
    }

    async fn extract(&mut self, i: usize, e: &ExtractSpec) -> Result<bool, Fail> {
        let name = e.name.clone().unwrap_or_else(|| format!("extract-{i}"));
        let missing = |optional: bool| {
            if optional {
                Ok(true)
            } else {
                Err((reason::TARGET_NOT_FOUND, None))
            }
        };
        let from = Some(self.url().await);
        if let Some(c) = &e.cards {
            let spec = json!({
                "selector": self.fill(&c.selector),
                "fields": c.fields,
                "max": c.max.unwrap_or(CARDS_MAX),
            });
            let out = self.call("pnexCards", spec).await;
            let cards: Vec<BTreeMap<String, String>> =
                serde_json::from_value(out["cards"].clone()).unwrap_or_default();
            if cards.is_empty() {
                return missing(e.optional);
            }
            let text = serde_json::to_string(&cards).unwrap_or_default();
            self.push(RunExtract {
                name,
                kind: ExtractKind::Cards,
                chars: text.chars().count() as u64,
                sha256: crate::sha256_hex(text.as_bytes()),
                text: text.chars().take(EXTRACT_MAX).collect(),
                from,
                file: None,
                bytes: None,
                cards,
            });
            return Ok(false);
        }
        let text = if let Some(sel) = &e.selector {
            if self.find(json!({ "selector": self.fill(sel) })).await.0 == 0 {
                return missing(e.optional);
            }
            self.call("pnexSelectorText", json!(self.fill(sel)))
                .await
                .as_str()
                .unwrap_or_default()
                .to_string()
        } else if let Some([a, b]) = &e.between {
            match between(&self.text().await, &self.fill(a), &self.fill(b)) {
                Some(t) => t,
                None => return missing(e.optional),
            }
        } else {
            self.text().await
        };
        self.push(RunExtract {
            name,
            kind: ExtractKind::Text,
            chars: text.chars().count() as u64,
            sha256: crate::sha256_hex(text.as_bytes()),
            text: text.chars().take(EXTRACT_MAX).collect(),
            from,
            file: None,
            bytes: None,
            cards: Vec::new(),
        });
        Ok(false)
    }

    async fn assert(&mut self, a: &AssertSpec) -> Result<bool, Fail> {
        let ok = if let Some(t) = &a.page_contains {
            contains_loose(&self.text().await, &self.fill(t))
        } else if let Some(n) = &a.not_empty {
            self.extracts
                .iter()
                .any(|e| &e.name == n && !e.text.trim().is_empty())
        } else if let Some(c) = &a.count_min {
            self.call("pnexCount", json!(self.fill(&c.selector)))
                .await
                .as_u64()
                .unwrap_or(0)
                >= u64::from(c.min)
        } else if let Some(u) = &a.url_contains {
            self.url().await.contains(&self.fill(u))
        } else {
            false
        };
        if ok {
            Ok(false)
        } else {
            Err((reason::ASSERT_FAILED, None))
        }
    }

    /// Each link of a cards extract, its sub-steps played on it (D790).
    async fn for_each(&mut self, i: usize, f: &ForEachSpec) -> Result<bool, Fail> {
        let links: Vec<String> = self
            .extracts
            .iter()
            .find(|e| e.name == f.from)
            .map(|e| {
                e.cards
                    .iter()
                    .filter_map(|c| c.get(&f.link).filter(|l| !l.is_empty()).cloned())
                    .collect()
            })
            .unwrap_or_default();
        let max = f.max.unwrap_or(FOR_EACH_MAX) as usize;
        for (n, link) in links.into_iter().take(max).enumerate() {
            let n = n + 1;
            if !self.on_site(&link) {
                return Err((reason::OFF_SITE, Some(link)));
            }
            // A reader's pace between two pages of the list (D258).
            host::sleep(2_000.0 + js_sys::Math::random() * 3_000.0).await;
            self.open(&link).await?;
            for (j, sub) in f.steps.iter().enumerate() {
                let sub = sub.numbered(j, n);
                let started = host::now_ms();
                let outcome = Box::pin(self.step(j, &sub)).await;
                self.report(i, &sub, &outcome, started, Some(n)).await;
                outcome?;
            }
        }
        Ok(false)
    }
}
