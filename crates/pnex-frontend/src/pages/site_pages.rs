//! Site crawls in the browser agents page (partner-watch.md D307–D310):
//! the `crawl_site` fields of the mission form, and the pages of a crawled
//! site with their proofs and the check of the proof chain.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::browser_agent::{
    CrawlExecutor, CrawlSiteParams, MatchKind, Matcher, Mission, PageRecord, SitePage, WebProof,
    CRAWL_DEPTH_MAX, CRAWL_PAGES_DEFAULT,
};

use crate::api;
use crate::components::modal::Modal;
use crate::components::ontology::short_time;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};

/// Form state of a `crawl_site` mission.
#[derive(Debug, Clone, PartialEq)]
pub struct CrawlDraft {
    pub agent: bool,
    pub max_pages: String,
    pub exclude: String,
    pub rules: String,
    pub proofs: bool,
    pub record: String,
}

impl CrawlDraft {
    pub fn of(p: Option<&CrawlSiteParams>) -> Self {
        match p {
            None => Self {
                agent: false,
                max_pages: CRAWL_PAGES_DEFAULT.to_string(),
                exclude: String::new(),
                rules: String::new(),
                proofs: true,
                record: String::new(),
            },
            Some(p) => Self {
                agent: p.executor == CrawlExecutor::Agent,
                max_pages: p.max_pages.to_string(),
                exclude: p.exclude.join("\n"),
                rules: p
                    .matchers
                    .iter()
                    .map(|m| match m.kind {
                        MatchKind::Regex => format!("re:{}", m.value),
                        _ => m.value.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                proofs: p.proofs,
                record: p
                    .record
                    .as_ref()
                    .and_then(|r| serde_json::to_string_pretty(r).ok())
                    .unwrap_or_default(),
            },
        }
    }

    /// One rule per line: `re:` = a regular expression, a line with a space
    /// = a phrase, else a word.
    pub fn params(&self, url: String) -> CrawlSiteParams {
        let matchers = self
            .rules
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| match l.strip_prefix("re:") {
                Some(r) => Matcher {
                    kind: MatchKind::Regex,
                    value: r.trim().to_string(),
                },
                None if l.contains(' ') => Matcher {
                    kind: MatchKind::Phrase,
                    value: l.to_string(),
                },
                None => Matcher {
                    kind: MatchKind::Word,
                    value: l.to_string(),
                },
            })
            .collect();
        CrawlSiteParams {
            url,
            executor: if self.agent {
                CrawlExecutor::Agent
            } else {
                CrawlExecutor::Server
            },
            max_pages: self.max_pages.trim().parse().unwrap_or(CRAWL_PAGES_DEFAULT),
            max_depth: CRAWL_DEPTH_MAX,
            exclude: self
                .exclude
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
            matchers,
            proofs: self.proofs,
            record: serde_json::from_str::<PageRecord>(self.record.trim()).ok(),
        }
    }

    pub fn record_invalid(&self) -> bool {
        !self.record.trim().is_empty()
            && serde_json::from_str::<PageRecord>(self.record.trim()).is_err()
    }
}

#[component]
pub fn CrawlFields(draft: Signal<CrawlDraft>) -> Element {
    let mut draft = draft;
    let d = draft();
    rsx! {
        div {
            label { class: FIELD_LABEL, {t!("crawl-field-executor")} }
            select {
                class: INPUT,
                onchange: move |e| draft.with_mut(|d| d.agent = e.value() == "agent"),
                option { value: "server", selected: !d.agent, {t!("crawl-executor-server")} }
                option { value: "agent", selected: d.agent, {t!("crawl-executor-agent")} }
            }
            p { class: FIELD_HELP, {t!("crawl-field-executor-help")} }
        }
        div {
            label { class: FIELD_LABEL, {t!("crawl-field-max-pages")} }
            input {
                class: INPUT,
                r#type: "number",
                min: "1",
                max: "2000",
                value: "{d.max_pages}",
                oninput: move |e| draft.with_mut(|d| d.max_pages = e.value()),
            }
        }
        div {
            label { class: FIELD_LABEL, {t!("crawl-field-rules")} }
            textarea {
                class: "{INPUT} h-24 font-mono",
                value: "{d.rules}",
                oninput: move |e| draft.with_mut(|d| d.rules = e.value()),
            }
            p { class: FIELD_HELP, {t!("crawl-field-rules-help")} }
        }
        div {
            label { class: FIELD_LABEL, {t!("crawl-field-exclude")} }
            textarea {
                class: "{INPUT} h-16 font-mono",
                placeholder: "/blog/",
                value: "{d.exclude}",
                oninput: move |e| draft.with_mut(|d| d.exclude = e.value()),
            }
        }
        label { class: "flex items-center gap-2 text-sm text-ink",
            input {
                r#type: "checkbox",
                checked: d.proofs,
                onchange: move |e| draft.with_mut(|d| d.proofs = e.checked()),
            }
            {t!("crawl-field-proofs")}
        }
        div {
            label { class: FIELD_LABEL, {t!("crawl-field-record")} }
            textarea {
                class: "{INPUT} h-24 font-mono",
                value: "{d.record}",
                oninput: move |e| draft.with_mut(|d| d.record = e.value()),
            }
            if d.record_invalid() {
                p { class: "mt-1.5 text-xs text-alarm", {t!("crawl-field-record-invalid")} }
            }
            p { class: FIELD_HELP, {t!("crawl-field-record-help")} }
        }
    }
}

fn state_label(state: &str) -> String {
    match state {
        "relevant" => t!("crawl-page-relevant").to_string(),
        "captured" => t!("crawl-page-captured").to_string(),
        "gone" => t!("crawl-page-gone").to_string(),
        _ => t!("crawl-page-listed").to_string(),
    }
}

#[component]
pub fn SitePagesDialog(mission: Mission, on_close: EventHandler<()>) -> Element {
    let id = mission.id.clone();
    let pages = use_resource(move || {
        let id = id.clone();
        async move { api::extension::site_pages(&id).await }
    });
    let mut check = use_signal(|| None::<String>);
    let open = use_signal(|| None::<String>);
    let verify = move |_| {
        spawn(async move {
            let text = match api::extension::verify_proofs().await {
                Ok(c) if c.ok => t!("crawl-chain-ok", count : c.count).to_string(),
                Ok(c) => {
                    t!("crawl-chain-broken", seq : c.broken_at.unwrap_or_default()).to_string()
                }
                Err(e) => crate::api::error_i18n::localize(&e),
            };
            check.set(Some(text));
        });
    };
    let list: Vec<SitePage> = match &*pages.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let status = match &*pages.read() {
        None => Some(t!("common-loading").to_string()),
        Some(Err(e)) => Some(crate::api::error_i18n::localize(e)),
        Some(Ok(l)) if l.is_empty() => Some(t!("crawl-pages-empty").to_string()),
        Some(Ok(_)) => None,
    };
    rsx! {
        Modal {
            title: t!("crawl-pages-title", name : mission.name.clone()).to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-3",
                div { class: "flex flex-wrap items-center gap-2",
                    button { class: BTN_SECONDARY_SM, onclick: verify, {t!("crawl-chain-verify")} }
                    if let Some(c) = check() {
                        span { class: "text-xs text-gray-700", "{c}" }
                    }
                }
                if let Some(s) = status {
                    p { class: "text-sm text-gray-500", "{s}" }
                }
                ul {
                    class: "divide-y divide-gray-100 text-sm",
                    "data-testid": "site-pages",
                    for p in list {
                        PageRow { key: "{p.id}", page: p, open }
                    }
                }
                if let Some(page) = open() {
                    ProofList { page_id: page }
                }
            }
        }
    }
}

#[component]
fn PageRow(page: SitePage, open: Signal<Option<String>>) -> Element {
    let mut open = open;
    let id = page.id.clone();
    let label = if page.title.is_empty() {
        page.url.clone()
    } else {
        page.title.clone()
    };
    let seen = t!(
        "crawl-page-seen", first : short_time(& page.first_seen_at), last : short_time(& page.last_seen_at)
    );
    rsx! {
        li { class: "py-2 space-y-1",
            div { class: "flex flex-wrap items-center gap-2",
                span { class: "rounded bg-gray-100 px-2 py-0.5 text-xs", {state_label(&page.state)} }
                a {
                    class: "text-signal-700 hover:underline break-all",
                    href: "{page.url}",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "{label}"
                }
            }
            p { class: "text-xs text-gray-500", "{seen}" }
            if page.last_proof_id.is_some() {
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| open.set(Some(id.clone())),
                    {t!("crawl-page-proofs")}
                }
            }
        }
    }
}

#[component]
fn ProofList(page_id: String) -> Element {
    let pid = page_id.clone();
    let proofs = use_resource(move || {
        let id = pid.clone();
        async move { api::extension::page_proofs(&id).await.unwrap_or_default() }
    });
    let list: Vec<WebProof> = proofs.read().clone().unwrap_or_default();
    rsx! {
        div { class: "rounded-lg border border-gray-200 p-3 space-y-2",
            h4 { class: "text-sm font-medium", {t!("crawl-page-proofs")} }
            for p in list {
                div { key: "{p.id}", class: "space-y-1 text-xs",
                    p {
                        {
                            t!(
                                "crawl-proof-line", seq : p.seq, at : p.captured_at.clone().map(| t |
                                short_time(& t)).unwrap_or_default()
                            )
                        }
                    }
                    if let Some(sha) = &p.manifest_sha256 {
                        p { class: "font-mono break-all text-gray-500", "{sha}" }
                    }
                    if let Some(m) = &p.manifest {
                        pre { class: "max-h-48 overflow-auto rounded bg-gray-50 p-2 font-mono",
                            {serde_json::to_string_pretty(m).unwrap_or_default()}
                        }
                    }
                }
            }
        }
    }
}
