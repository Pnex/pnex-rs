# Notifications — abstraction multi-canaux, templates et nœud flow (D49–D54, D61–D65)

> **Statut : DOCTRINE (réflexion tranchée le 2026-09-13, implémentation par
> phases — §11). AMENDEMENTS 2026-09-14 (implémentation N1–N3, worktree
> `notifications`) :** ntfy retiré et smtp reporté — le registre v1 ne
> livre que **`websocket`** (bus interne `/ws/notify` + endpoint interne
> `/internal/notify/deliver`, jeton de service `settings.notifications`)
> et **`webhook`** (POST générique — couvre ntfy/Gotify/Telegram/domotique,
> qui ne sont que des POST ; ntfy/smtp dédiés restent des extensions
> ~30 lignes). **Pas de centre de notifications dans le front PNeX** (l'app
> a déjà ses toasts ; le websocket est un canal configurable consommé par
> des clients externes). **Push natif FCM/APNs reporté** : produit
> self-hosté à distribution unique ⇒ creds push configurables par instance
> (point d'extension). Reporting engine→backend pour le journal websocket :
> partiellement tiré de N6 (endpoint interne + journal source `flow`).
> AMENDEMENT N4 (2026-09-14, §13) : canaux dédiés ntfy, telegram, slack,
> discord, smtp (D61–D65) + picker de kind + garde i18n registry↔locales.
> Référence pour tout le
> chantier notification : choix de l'abstraction (D49), modèle à trois
> objets org-scoped avec résolution au déploiement (D50), registre de
> canaux extensible (D51), templates minijinja (D52), sémantique de
> livraison (D53), secrets (D54). Sources amont :
> `docs/architecture/flow-engine.md` (save ≠ déployé, nœuds `pnex_*`),
> `docs/architecture/edge-model.md` (D44–D48), pattern registre
> `ai_connectors` (migration 000010).

## 1. Le problème

Les événements applicatifs de PNeX (seuil dépassé, job de stitch terminé,
device offline, fin de provisioning…) doivent pouvoir rejoindre des
systèmes externes : ntfy, e-mail, Telegram, Slack, Discord, webhooks
domotique. Aujourd'hui : rien. L'étude de l'écosystème (Apprise,
Shoutrrr, shoutrrr-rs) conclut :

| Option | Verdict |
|---|---|
| **Apprise** (Python) | standard de fait (60+ services) mais Python : un sidecar de plus à opérer — exclu (pas de microservice de plus) |
| **Shoutrrr** (Go) | bon design URL-based, mais lib Go — non liable depuis pnex |
| **shoutrrr-rs** | le seul port Rust, archi saine (runtime-agnostic, transport en trait) mais **3 canaux** (Slack/Discord/webhook) — il faudrait contribuer tous les canaux utiles sur une crate jeune |
| **Trait maison + crates éprouvées** | ntfy/Telegram/Slack/Discord/Gotify = un `reqwest::post` chacun (~30 l. chacun) ; SMTP = `lettre` ; templates = minijinja. Le périmètre utile est 5–7 canaux, pas 60 |

**D49 — abstraction en lib maison.** Un trait `Channel` + une crate
partagée `pnex-notify` (zéro dep loco), utilisée par le backend (envois
directs, tests UI) et par le nœud flow (envois depuis edgelinkd). La
couverture Apprise (60 services) n'est pas un objectif ; le périmètre
cible est 5–7 canaux éprouvés. Pas de microservice de plus à côté de
Rauthy et OpenObserve.

## 2. Le modèle : trois objets, org-scoped (D50)

Trois objets métier, tous scopés par `org_id` (couche D42, header
`X-Org-Id`) :

| Objet | Table (DSL loco, migration 000020) | Contenu |
|---|---|---|
| **Canal** (destination) | `notify_channel` → `notify_channels` | `kind` (ntfy, webhook, smtp…), `name` unique par org, `config` JSON typé par kind (contient les secrets), `enabled` |
| **Template** | `notify_template` → `notify_templates` | `name` unique par org, `subject` (optionnel, minijinja), `body` (minijinja), `vars` déclarées `[{name, example}]` pour le picker UI et l'aperçu |
| **Nœud flow `pnex_notify`** | config du nœud dans le graphe | `channel_ids` (1..n), `template_id`, `vars` (overrides), `strict: bool` |

**Résolution au déploiement (snapshot).** Le nœud du graphe ne porte que
des références (`channel_ids`, `template_id`). **Au déploiement**, le
backend résout les références et injecte dans la config du nœud déployé
un snapshot `{kind, config, subject, body}`. Conséquences assumées :

- cohérent avec la doctrine « version sauvegardée ≠ déployée » :
  éditer un canal ou un template ne mute pas les flows déjà déployés —
  **re-déployer le flow pour propager** ;
- le backend ne liant jamais le moteur, il n'y a **pas** d'aller-retour
  engine→backend au send : le snapshot rend le nœud autonome ;
- le toast de deploy liste les nœuds `pnex_notify` dont les références
  sont périmées (snapshot ≠ config courante du canal/template), à
  l'image de la chip runtime déjà pollée 5 s ;
- rotation d'un secret côté canal ⇒ re-déployer les flows qui
  l'utilisent.

## 3. Registre des canaux (D51)

Chaque kind = une implémentation du trait (dans `pnex-notify`) :

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    /// identifiant du kind, ex. "ntfy"
    fn kind(&self) -> &'static str;
    /// spécification des champs de config (source du formulaire UI)
    fn field_spec() -> Vec<FieldSpec>;
    /// validation de la config à la sauvegarde
    fn validate(config: &serde_json::Value) -> Result<(), String>;
    async fn send(cfg: &serde_json::Value, msg: &Message) -> Result<(), NotifyError>;
}
```

Chaque kind = « 1 variante + 1 impl du trait + 1 `field_spec` + 2 clés
i18n » — même économie de points d'extension que le registre des
attachments POI.

`FieldSpec` décrit un champ de formulaire
(`id, label_i18n, type: text|secret|number|select|bool, required, placeholder, help_i18n`).
Le backend expose `GET /api/notify/kinds` → `[{kind, label, field_spec}]` :
le front rend le formulaire d'édition canal **depuis le backend** (source
unique de vérité, zéro drift front/back pour un produit à formulaires) ;
les labels/icons restent locaux au front (registre front pour l'icône,
i18n `notify-kind-*`).

Canaux (ordre du registre = ordre du picker) :

| Kind | Transport | Champs |
|---|---|---|
| `websocket` | bus interne `/ws/notify` + `/internal/notify/deliver` | aucun (creds d'instance par env) |
| `webhook` | reqwest POST JSON `{subject, body, meta}` | `url`, `secret_header`/`secret_value` (secret) |
| `ntfy` | reqwest publication JSON `{topic, message, title, priority, tags}` | `server`, `topic`, `token` (secret), `priority` (select), `tags` |
| `telegram` | Bot API `sendMessage` | `bot_token` (secret), `chat_id` |
| `slack` | incoming webhook `{"text"}` | `webhook_url` (secret) |
| `discord` | webhook `{"content"}` | `webhook_url` (secret), `username` |
| `smtp` | lettre 0.11 (rustls) | `host`, `port`, `tls` (select starttls/tls/none), `username`, `password` (secret), `from`, `to` |
| `gotify` | reqwest POST | `url`, `app_token` (secret), `priority` — en réserve |

N4 livré : ntfy, telegram, slack, discord, smtp (D61–D65) ; gotify
reste en réserve (~30 l. quand le besoin arrive).

## 4. Templates minijinja (D52)

**minijinja** (pas tera : plus petit, embeddable côté nœud sans tirer un
stack de mailer, sandbox par conception, filtres custom faciles). Le
moteur de rendu vit dans `pnex-notify`, partagé backend + nœud.

Contexte de rendu :

```json
{
  "msg":  { "payload entrant du flow (objet quelconque)" },
  "meta": { "flow": "…", "node": "…", "ts": "…", "org": "…" },
  "vars": { "overrides du nœud, pré-rendus contre {msg, meta}" }
}
```

Le payload entrant est **aussi** exposé à la racine du contexte
(`{{ temperature }}` = `{{ msg.temperature }}`) pour l'ergonomie ;
en cas de collision, la var explicite gagne.

Les `vars` rendues suivent la même école : référençables avec ou sans
préfixe (`{{ seuil }}` = `{{ vars.seuil }}` — l'éditeur de templates
laisse écrire les deux). Précédence en collision : clé payload < var
racine < namespace explicite (`msg`/`meta`/`vars` — une var nommée
`meta` ne masque jamais le namespace).

**Composition à deux niveaux** : les `vars` du nœud sont rendues
d'abord contre `{msg, meta}`, puis le template est rendu contre
`{msg, meta, vars}`. Une var peut donc être un littéral `"80"` ou une
référence `"{{ msg.value }}"`.

```jinja
{# corps type #}
⚠️ {{ device }} : {{ value }} {{ unit }} (seuil {{ vars.seuil }}, {{ meta.ts }})
```

**Bornes de rendu** : minijinja est sandbox par conception (pas
d'accès disque, pas d'include arbitraire) ; les seules bornes à poser
soi-même : longueur de sortie bornée 64 KiB et profondeur limitée — un
template utilisateur peut s'auto-inonder.

**Preview API** : `POST /api/notify/templates/:id/preview` rend avec les
`example` des vars déclarées (ou un payload fourni), **sans envoi**. Le
bouton Test d'un template (envoi réel sur un canal choisi) emprunte le
même chemin pnex-notify que le nœud flow — pas de code d'envoi dupliqué.

## 5. Sémantique de livraison (D53)

- **at-least-once** : retry 3× (backoff 1 s / 5 s / 25 s), timeout 10 s
  par tentative (total borné ~40 s).
- **Nœud flow lenient par défaut** : un échec de notification ne fait
  **pas** tomber le flow (tracing warn + entrée journal `failed`) ;
  `strict: true` → échec = `flow_error` isolé (philosophie ferme
  d'engines : jamais de crash-loop).
- **Journal `notify_deliveries`** — *remplacé par D86 (§15) : le journal
  vit dans OpenObserve, la table est supprimée* : `(id, org_id, channel_id,
  template_id?, source: flow|direct|test, status: sent|failed, error?,
  ts, flow_run?)`, pruning 30 j (worker loco — précédent : worker de
  build firmware). Badge « dernier statut » sur la carte canal de l'UI.
  v1 : le journal couvre les envois backend + tests ; le nœud flow
  journalise côté moteur (tracing + debug feed) ; un endpoint interne
  engine→backend de reporting est un point d'extension (N6, nécessite
  un jeton de service — hors périmètre v1).
- **Test** : `POST /api/notify/channels/:id/test` envoie un message de
  test rendu depuis un template built-in ; `POST
  /api/notify/channels/test-draft` permet de tester un brouillon avant
  sauvegarde. Les deux passent par pnex-notify.

## 6. Secrets (D54)

- **Masqués en API/UI** : les champs déclarés `secret` dans le
  `field_spec` sont write-only — PUT les accepte, GET ne les rend jamais
  (le front affiche « défini » + bouton « remplacer »). Pattern
  `deserialize_some` déjà connu du front.
- **Remplacé par le coffre de secrets (2026-10-01, `secrets.md` lot S4)** :
  les champs `secret` sont des références vers `org_secrets`
  (XChaCha20-Poly1305, clé hors base), le snapshot ne porte que des
  `secret_id` et le runtime résout via `/internal/flow/secret/{id}` ; la
  clé maîtresse ne va jamais dans le runtime. Le paragraphe ci-dessous est
  l'intention d'origine, conservée pour l'historique.
- **Chiffrement at rest en phase N5** : AEAD XChaCha20-Poly1305, clé
  maître en config/env **partagée backend ↔ edgelinkd** (le snapshot du
  nœud transporte le chiffré, l'engine déchiffre au send). Avant N5 :
  plaintext en DB et dans `flow-state/flows.json` — fichiers
  serveur-locaux, risque documenté et accepté en v1. (NB : `chacha20`
  nu est déjà dep backend pour les tokens device — D8, parité avec le
  POC initial ;
  ici il faut un AEAD, donc `chacha20poly1305` en plus, feature-gated
  dans pnex-notify.)
- **Jamais de secret dans les URLs ni les logs** : les URLs sont
  loggées, les champs `secret` jamais ; le token est toujours un champ
  secret séparé, jamais concaténé dans une URL.

## 7. API (loco, org-scoped `X-Org-Id`, gated `can_write`)

```
GET    /api/v1/notify/kinds                          → [{kind, label, field_spec}]
GET    /api/v1/notify/channels                       → liste (secrets masqués)
POST   /api/v1/notify/channels                       → création (secrets write-only)
GET    /api/v1/notify/channels/:id                   → détail (secrets masqués)
PUT    /api/v1/notify/channels/:id
DELETE /api/v1/notify/channels/:id
POST   /api/v1/notify/channels/test-draft            {kind, config} → test d'un brouillon
POST   /api/v1/notify/channels/:id/test              → message de test (template built-in)
GET    /api/v1/notify/channels/:id/deliveries        ?limit=50 (journal O2 du canal, D86)
GET    /api/v1/notify/deliveries                     ?channel_id&status&source&q&from&to (D86)
POST   /internal/notify/journal                      jeton interne — 1 tentative du nœud flow (D86)
GET    /api/v1/notify/templates
POST   /api/v1/notify/templates
GET    /api/v1/notify/templates/:id
PUT    /api/v1/notify/templates/:id
DELETE /api/v1/notify/templates/:id
POST   /api/v1/notify/templates/:id/preview          {vars?} → rendu seul, jamais d'envoi
```

Règle : **le test contourne `enabled: false`** (c'est son but) ; en
revanche, au déploiement, les canaux `enabled: false` référencés par un
nœud `pnex_notify` sont ignorés et listés dans le toast de deploy.

## 8. UI (page `/notifications`)

Deux onglets (navigation principale → « Notifications ») :

**Onglet Connect** — liste de cartes canal (icône kind, nom, enabled,
badge dernier statut de livraison) :
- **Nouveau** → dialogue : picker de kind (grille d'icônes) → formulaire
  dynamique généré depuis `GET /api/notify/kinds` (field_spec) ; secrets
  write-only ; bouton **Test** (test-draft) avant même la sauvegarde ;
- édition = même formulaire pré-rempli, champs secret affichés
  « défini » avec bouton « remplacer » ;
- toggle enabled + Test sur carte ; bouton **Journal** → onglet
  Événements filtré sur le canal (D86).

**Onglet Modèles** — liste + éditeur (nom, sujet, corps minijinja, vars
déclarées `{name, example}` avec aide à la saisie `{{ }}`), bouton
**Aperçu** (preview API), bouton **Test d'envoi** (canal choisi dans un
dialogue, chemin pnex-notify unique).

**Éditeur de flows** : inspecteur du nœud `pnex_notify` (nouvelle branche
d'inspecteur par kind — pattern inject/debug/red) :
- multi-select canaux de l'org + select template ;
- éditeur de `vars` : lignes clé/valeur, la valeur est un littéral **ou**
  une expression `{{ msg.x }}` pré-rendue contre le payload entrant ;
- switch `strict` ;
- payload d'exemple éditable + bouton Aperçu (même preview API).

## 9. Payoffs / pièges

Payoffs :
- un chemin d'envoi unique (pnex-notify) pour backend, tests UI et nœud
  flow — pas de code d'envoi dupliqué ;
- nouveau canal = 1 variante + 1 impl + 1 field_spec + 2 clés i18n —
  même économie que le registre des attachments POI ;
- snapshot au deploy = nœud autonome, pas d'engine→backend au send,
  cohérent avec « save ≠ déployé » ;
- le trio ntfy/webhook/SMTP couvre le self-host domotique ; ntfy apporte
  le push téléphone sans infra de push propriétaire.

Pièges consignés :
- **pluraliseur loco** : DSL `notify_channel` → table `notify_channels`
  (piège déjà mordu avec `resource_containments`) ;
- **clé fluent manquante = panic** — sweep `t!()` avant commit (i18n
  `notify-*`) ;
- **deploy ≠ save** : un template/canal édité ne se propage pas aux
  flows déployés — d'où le toast de périmés au deploy ;
- le front construit le formulaire depuis `GET /kinds` : toute évolution
  de `FieldSpec` est un changement de contrat front↔back — versionner
  `notify-kinds.v1`, évolution additive-only (école CONTRACT).

## 10. Non-goals

- alerting infra (seuils métriques) = **OpenObserve alert destinations**,
  qui fait déjà le fan-out (Slack/webhook/email) — pnex-notify couvre
  les événements applicatifs et les flows ;
- couverture Apprise (60+ services) — 5–7 canaux éprouvés suffisent ;
- webhooks **entrants** (trigger de flow par HTTP externe) — point
  d'extension documenté, pas de phase ;
- préférences de notification par utilisateur — PNeX est org-scoped
  pour l'exploitation.

## 11. Phases

| Phase | Contenu | Critère de sortie |
|---|---|---|
| **N1** ✅ | crate `pnex-notify` (trait + minijinja + **websocket/webhook**), tables **[000021]**, CRUD API + tests + journal backend + bus WS + endpoint interne | 21 tests crate + 8 intégration (masquage, WS, journal) |
| **N2** ✅ | UI `/notifications` : onglet Connect (formulaires dynamiques field_spec, test, masquage secrets) + Modèles (éditeur, preview, test) — **pas de centre de notifications dans l'app** (décision 2026-09-14) | E2E web headless (login scripté Rauthy) |
| **N4** ✅ | **canaux dédiés ntfy, telegram, slack, discord, smtp** (D61–D65) + picker de kind à la création (grille, doctrine §8) + garde i18n registry↔locales | 41 tests crate (7 kinds) + e2e (contrat field_spec, masquage D54 sur un 2e kind, test-draft ntfy) ; testés en réel par le produit |

> **Exploitation (constaté E2E 2026-09-14)** : le canal websocket des nœuds
> flow exige `PNEX_NOTIFY_INTERNAL_TOKEN` (env du serveur, fail-closed sans)
> — le supervisor l'injecte au runtime avec `PNEX_NOTIFY_DELIVER_URL`. Le
> jeton est accepté par `/internal/notify/deliver` en `x-pnex-internal-token`
> **ou** `Authorization: Bearer` (le canal pnex-notify poste en Bearer).
| N5 | chiffrement AEAD des secrets, clé env partagée backend↔engine | rotation de clé documentée — **livré par `secrets.md` S4 (coffre, sans clé dans le runtime)** |
| N6 | (points d'extension) reporting engine→backend pour le journal, webhooks entrants | — |

## 12. Sources de l'étude amont

- Apprise (Python, 60–100+ services, schéma d'URL par service) —
  standard de fait, non-Rust.
- Shoutrrr (Go) — design URL-based, utilisé par Watchtower/Kured.
- shoutrrr-rs (connyay/shoutrrr-rs) — port Rust à 3 canaux, archi
  runtime-agnostic + transport en trait — point de départ envisagé puis
  écarté (couverture insuffisante).
- ferro-notifications — plus de canaux mais couplé au framework Ferro,
  churn 0.2.x — écarté.
- minijinja — moteur de templates sandbox par conception.
- lettre 0.11 — SMTP Rust de référence, rustls.

## 13. Amendement N4 — canaux dédiés (D61–D65, 2026-09-14)

Choix produit : tous les systèmes de notification visibles dans le picker
avec leurs champs propres (doctrine §8 appliquée) — l'utilisateur ne
devine jamais comment configurer un service. Livré : ntfy, telegram,
slack, discord, smtp.

- **D61 — Registre à 7 canaux, creds par canal.** ntfy/telegram/slack/
  discord/smtp sont auto-porteurs : toute la config vit dans `config`
  JSONB du canal (secrets write-only D54) — **jamais** de creds d'instance
  env (le chemin `apply_runtime_env` reste l'exception websocket). Kinds
  strings exacts : `ntfy`, `telegram`, `slack`, `discord`, `smtp`. Le
  champ ntfy s'appelle `server` (le croquis §3 disait `url` : l'endpoint
  est composé `{server}/{topic}` → publication JSON sur `{server}/`).
  Les valeurs des selects (`priority`, `tls`) sont techniques, rendues
  telles quelles par le formulaire générique (entériné).
- **D62 — Projection du Message.** `subject` → titre du canal (objet de
  l'e-mail, première ligne du texte telegram/slack/discord, `title` ntfy) ;
  `body` → texte brut ; **`meta` jamais projeté** (le template a déjà
  `{{ meta.* }}` à disposition — projeter meta masquerait l'origine).
  Composition `"{subject}\n{body}"` via le helper partagé `compose_text`.
- **D63 — Bornes de taille fail-loud, pas de troncature.** telegram 4096
  chars, discord 2000 chars → `NotifyError::TooLarge` (non retryable) via
  `size_guard` ; ntfy exempt (son serveur attache en pièce jointe au-delà
  de 4096 octets au lieu de rejeter) ; slack/smtp délèguent au serveur.
  Cohérent avec la borne de rendu 64 KiB ; une notification tronquée en
  silence est dangereuse pour l'alerting.
- **D64 — SMTP lettre 0.11 rustls-only.** `default-features = false`
  (le défaut tire native-tls) + `tokio1-rustls`/`rustls` +
  `rustls-native-certs` (CA privées des relais self-hostés) + `ring`
  (cohérence reqwest). Modes : `tls` → `relay` (465), `starttls` →
  `starttls_relay` (587, défaut), `none` → `builder_dangerous` (25 —
  **pas d'`unencrypted_relay`**, ça n'existe pas). Ports toujours posés
  explicitement. Erreurs lettre : `is_transient()/is_timeout()` →
  `Network` (retryable), sinon → `Config` (un 535 permanent ne se répare
  pas au retry).
- **D65 — URL porteuse de secret + garde i18n registry↔locales.**
  slack/discord : l'URL du webhook **est** le secret (token dans le
  chemin) → classée champ `secret` (write-only, masquée, jamais loggée) —
  exception assumée à D54. Et : les `label_i18n`/`help_i18n` des
  field_spec ne passent pas par `t!("littéral")` — le sweep front ne les
  voit pas (une clé manquante = panic à l'ouverture du formulaire : les 3
  clés webhook manquaient). Garde : test `cles_field_spec_presentes_dans_les_locales`
  dans `registry.rs` qui inclut les deux .ftl et échoue si une clé du
  field_spec manque (ferme la classe de bug ; la garde ntfy avait d'abord
  montré la lacune).

Pièges à consigner : ntfy publie **en JSON** (`{topic, message, title…}`
vers `{server}/`) et non en headers `X-Title` — un sujet de template est
UTF-8 arbitraire et un header HTTP non-ASCII casse la requête ; les
messages Fluent **échappent les accolades** — une desc i18n contenant
`{…}` est un placeable, la locale entière est rejetée (tombée au 1er
lancement).

## 14. Amendement N5 — ancres canvas + port trigger obligatoire (D66–D68, 2026-09-24)

Deux manques livrés ensemble : les ancres d'entrée par var de template
n'étaient **pas rendues** sur le canvas (la géométrie les calculait, le
rendu canvas ne les connaissait pas — un seul point mid-height), et
l'envoi partait à chaque message complétant le set, soit du spam potentiel
sur une source rapide.

- **D66 — Ancres canvas = un arm par rangée labellisée.** Le match
  `in_anchor_rows` de `canvas.rs` ne couvrait que device-write / fonction /
  json-merge ; notify tombait dans le cas fourre-tout. Arm `PnexNotify`
  ajouté (zip rangées ↔ labels) : le reste du pipeline (fils annotés,
  reverse-wiring, prune) étant générique et piloté par les labels, les
  ancres vars apparaissent dès le stamp du pick (effet inspecteur).
- **D67 — Port `trigger` booléen OBLIGATOIRE (anti-spam).** Le nœud
  gagne une ancre permanente `trigger` (1ʳᵉ rangée, au-dessus des vars) :
  une fonction logique (comparaison, seuil…) y branche sa sortie booléenne
  et **décide** de l'envoi. Sémantique runtime : un message `topic =
  "trigger"` est une mise à jour du gate — jamais des données template —
  et est **avalé** (pas de passthrough du booléen vers l'aval) ; les
  envois ne partent que gate armé ; un set complet non armé attend
  (`ready`), un `trigger = true` le commit. Coercion école Node-RED :
  booléens, nombres non nuls, chaînes `"true"/"false"/"1"/"0"`
  (insensibles à la casse) — tout le reste désarme (fail-safe). Le gate est
  **obligatoire** : `validate_graph` pose `notify_trigger_required` sur
  tout nœud notify sans câblage trigger (save bloquée), et le runtime
  **refuse de construire** un artefact sans le flag deploy-dérivé
  `pnex_notify_trigger` (école D50 « redéployez » — un artefact de l'ère
  always-send ne doit jamais pouvoir envoyer).
- **D68 — Nom `trigger` réservé (si câblé).** Une var de template nommée
  littéralement `trigger` partagerait le tagger `topic = "trigger"` du
  gate : violation `notify_trigger_conflict` (renommer la var dans le
  template). Non câblé, le nom reste utilisable — mais la save exige
  alors le câblage (D67), donc le cas réel est couvert par D68.

Routage : le deploy réutilise les taggers — toute annotation
`pin = "trigger"` sur un nœud notify reçoit un tagger `topic = "trigger"`
comme une var, et la projection estampe `pnex_notify_trigger: true` sur
l'entrée artefact (dérivé du graphe, zéro état UI, self-healing au
redéploiement). Anti-spam et passthrough inchangés ; le msg de commit
(trigger) ne passe pas non plus vers l'aval.

**Amendement 2026-09-30 (observations O10/O11).** Deux corrections de
sémantique, vécues en E2E réel (flow prédictif → ntfy) :

- **Dernière valeur gagnante** : un set en attente (gate désarmé ou
  anti-spam) est rafraîchi par chaque nouvelle valeur — « différé, jamais
  perdu » signifie que la notification finit par partir, **pas** avec des
  données figées au moment où le set est devenu complet (avant : alerte
  « ETA 0 min » envoyée 9 min plus tard alors que la prévision disait 17).
- **Commit différé (`COMMIT_GRACE` = 250 ms)** : un `trigger = true` sur
  un set en attente ne rend plus immédiatement ; le commit est planifié et
  exécuté par une tâche dédiée du nœud après la fenêtre de grâce. Les
  données qui arrivent pendant la fenêtre rafraîchissent le set sans
  envoyer seules. Motif : la même sortie amont alimente à la fois une var
  et la logique du trigger ; l'ordre d'arrivée du fan-out edgelink n'est
  pas garanti, et un trigger arrivé avant sa donnée envoyait le set du tick
  précédent (vibration 2.06 au lieu du pic 10.71). Coût : ≤ 250 ms de
  latence sur ce chemin ; un set complet arrivant gate déjà armé part
  toujours immédiatement.

## 15. Amendement D86 — journal des livraisons dans OpenObserve (2026-09-29)

Décision (demande produit : « pas besoin de pourrir psql/sqlite avec ça ») :
le journal des livraisons quitte la base relationnelle.

- **Stockage** : stream O2 logs `notify_deliveries` dans l'organisation O2
  de l'org (provisioning paresseux à la première écriture, comme la
  télémétrie). Table `notify_deliveries` **supprimée** (migration
  000037) ; les anciennes lignes ne sont **pas** migrées. Plus de pruner :
  la rétention est celle d'O2 (D72). OpenObserve est **obligatoire** dans
  la stack — sans O2, les lectures répondent `available: false` et les
  écritures sont perdues (log warn).
- **Document** (plat, O2 aplatit les objets) : `channel_id`,
  `channel_kind`, `template_id?`, `source` (`flow`|`test`|`ota`),
  `status` (`sent`|`failed`|`blocked`), `http_status?` (retour amont),
  `error?`, `subject?`, `flow_id?`, `node_id?`, `delivered?` (sessions WS
  atteintes), `message` (sujet + erreur — champ plein texte : `match_all`
  n'indexe que les clés FTS par défaut d'O2).
- **Écrivains — tout envoi est journalisé** :
  - websocket (bus interne) : `deliver_in_app` côté serveur — seul chemin,
    qu'il vienne d'un flow, du bouton Test (loopback, source `test` via
    `meta.test`) ou d'une issue OTA ;
  - autres canaux depuis un flow : le nœud `pnex-notify` poste chaque
    tentative (`sent`, `failed` + HTTP, `blocked` par l'anti-spam) sur
    `POST /internal/notify/journal` (même jeton que deliver, URL
    `PNEX_NOTIFY_JOURNAL_URL` injectée par le superviseur), en tâche
    détachée — le journal ne retarde ni ne fait échouer un envoi ; un
    websocket réussi n'est pas re-journalisé par le nœud (le serveur l'a
    fait) ;
  - boutons Test : `send_test` (sauf websocket réussi, journalisé par le
    loopback).
  Côté serveur, les écritures passent par un writer de fond (`submit`,
  spawné au boot) : les transitions OTA n'ont qu'une connexion DB.
- **Lecteurs** : `GET /api/v1/notify/deliveries` (enveloppe D14 +
  `available`, total par une requête `count(*)` séparée — le `total`
  d'O2 ne compte que la page) ; badge « dernier statut » des cartes canal
  = **une** requête agrégée `first_value(status ORDER BY _timestamp DESC)
  … GROUP BY channel_id` sur 30 jours (best effort : O2 indisponible =
  pas de badge, jamais une erreur).
- **UI** : 3e onglet **Événements** dans /notifications (canaux ·
  modèles · événements) — filtres canal / statut / source / période /
  plein texte, ligne dépliable (erreur, nœud, modèle, sessions atteintes),
  lien vers le flow.
- **Validé E2E réel (2026-09-29)** : serveur isolé (base dédiée, séquence
  d'org décalée pour ne pas re-provisionner les orgs O2 du stack de dev),
  O2 + Rauthy réels ; Test webhook OK / 404 / websocket, flow `inject →
  pnex-notify` vers 3 canaux (2 webhooks + bus) → 3 entrées `flow` avec
  `flow_id`, `node_id`, `template_id`, HTTP 404 ; badges ; UI fr/en
  (Playwright), bouton Journal préfiltré.

