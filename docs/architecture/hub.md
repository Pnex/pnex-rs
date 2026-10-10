# PRD — PNEX Hub : partage communautaire de kits

| | |
|---|---|
| **Statut** | 🧊 **Gelé** — implémentation suspendue jusqu'à la refacto ontologie (v0.2.0). Relu contre la doc le 2026-10-10 (§20) |
| **Licence** | MIT (projet) + DCO sur les contributions |
| **Dépendances bloquantes** | Noyau ontologique 0.2.0 (`ontology.md`, D176–D191), références inter-objets par UUID |
| **Entrée roadmap** | P3 (horizons), décision #23 |
| **Renvois** | `ontology.md` (D190 packs, D176 types), `secrets.md` (D110 références de coffre), `custom-firmware.md` (D87–D94), `flow-engine.md` (fonctions JS / Starlark), `security.md` (R1–R20), `pages.md` (modèles dans les packs) |

---

## 1. Contexte et problème

PNEX permet de construire des flows, dashboards, synoptiques, fonctions custom (JS / Starlark) et variantes de cartes. Aujourd'hui, tout ce qui est construit reste enfermé dans l'instance où il a été créé.

Il n'existe aucun moyen de :
- partager rapidement un travail fait en local ;
- installer un « kit prêt à l'emploi » (matériel + logique + visualisation) ;
- s'appuyer sur la communauté pour enrichir le catalogue.

C'est un frein direct à la stratégie communauté (projets d'un week-end sur Instructables/Medium, cartes PCB publiées en libre). Chaque tutoriel devrait pouvoir se terminer par « installe le kit en un clic ».

## 2. Objectifs

1. Un dev exporte et publie un objet ou un kit **en moins de 2 minutes** depuis son instance locale.
2. Le mainteneur valide les soumissions via un **workflow Git standard** (PR → revue → merge).
3. Un utilisateur installe un kit **sans quitter PNEX** et le rebinde à son matériel en quelques clics.
4. Un kit installé est **portable et robuste** : renommer, remplacer ou reconfigurer un device ou le réseau ne casse rien.
5. **Zéro backend marketplace** à opérer : Git + CI + index statique.

## 3. Non-objectifs (v1)

- Monétisation ou kits payants.
- Notation, commentaires ou système social (les issues/discussions GitHub suffisent).
- Synchronisation bidirectionnelle (un kit installé est une copie locale, pas un lien vivant).
- Partage de données de télémétrie ou de médias lourds.
- Hébergement des fichiers de fabrication KiCad dans le hub : on met un lien vers le repo hardware.

## 4. Personas

| Persona | Besoin |
|---|---|
| **Maker / dev contributeur** | Partager vite ce qu'il a construit, via CLI ou bouton |
| **Utilisateur maker** | Trouver un kit, l'installer, le brancher à son matériel — **depuis l'UI uniquement** |
| **Mainteneur (Shan)** | Revue rapide, CI qui filtre le bruit, confiance dans ce qui est mergé |
| **Org industrielle** | Hub privé interne, sans dépendre du hub public |

## 5. Concepts

| Concept | Définition |
|---|---|
| **Objet partageable** | Flow, dashboard, synoptique, fonction custom (JS / Starlark), variante de carte, modèle de page (`pages.md` P13) |
| **Kit** | Bundle versionné regroupant un ou plusieurs objets + manifeste + doc |
| **Slot** | Rôle typé déclaré par le kit (`temp_ext : analog_in, °C`). Remplace toute référence à un objet local |
| **Binding** | Association locale slot → objet de l'instance (par UUID ou sélecteur) |
| **Requirement** | Besoin de config de site (`network.wifi`, `mqtt.endpoint`, secret…), résolu localement |
| **Profil réseau / site** | Config locale (SSID, credentials, endpoints) injectée au build ou au provisioning |
| **Tap** | Source de kits = n'importe quel repo Git qui suit la structure du hub |

### Règle d'or

> **Un kit ne référence jamais un nom, un ID local, une IP, un SSID ou un secret. Seulement des slots et des requirements.**

## 6. Format du bundle

### 6.1 Arborescence du repo `Pnex/hub`

```
hub/
├── kits/
│   └── meteo-station-c6/
│       ├── kit.toml
│       ├── README.md
│       ├── objects/
│       │   ├── flow.ingest-meteo.json
│       │   ├── dashboard.meteo.json
│       │   └── board.pnex-c6-meteo.json
│       └── media/
│           └── cover.png
├── schemas/          # generated from pnex-core
└── .github/workflows/
```

### 6.2 Manifeste `kit.toml`

```toml
[kit]
id          = "meteo-station-c6"
name        = "Station météo ESP32-C6"
version     = "1.2.0"            # kit semver
authors     = ["jdoe <jdoe@example.org>"]
license     = "MIT"
pnex        = ">=0.2.0, <0.3.0"  # platform compatibility
tags        = ["météo", "extérieur", "c6"]
links       = { instructables = "https://…", hardware = "https://github.com/…/kicad" }

[[objects]]
kind           = "flow"
path           = "objects/flow.ingest-meteo.json"
schema_version = 3

[[objects]]
kind           = "dashboard"
path           = "objects/dashboard.meteo.json"
schema_version = 2

[[slots]]
name        = "temp_ext"
capability  = "analog_in"
unit        = "°C"
description = "Sonde de température extérieure"
cardinality = "one"              # one | many

[[slots]]
name        = "capteurs_serre"
capability  = "analog_in"
cardinality = "many"
selector    = { tags = ["serre"] }   # suggested selector binding

[[requirements]]
key  = "network.wifi"
kind = "network_profile"

[[requirements]]
key  = "notif.email"
kind = "secret"
optional = true
```

### 6.3 Références dans les objets

À l'intérieur des JSON, toute référence externe prend la forme `{"$slot": "temp_ext"}` ou `{"$req": "network.wifi"}`. Les références **internes au kit** (le dashboard pointe sur le flow du même kit) utilisent `{"$local": "flow.ingest-meteo"}`.

## 7. Export (instance → bundle)

**UI (chemin utilisateur)** : bouton « Exporter en kit » sur chaque objet ; il permet d'éditer les slots avant d'écrire le bundle (téléchargement d'une archive).

**CLI (contributeurs, automatisation)** : `pnex kit export <type>/<uuid> [--with-deps] -o ./mon-kit` — même service que le bouton, jamais un chemin exclusif.

1. Lit la **version courante** de l'objet en Postgres (`flow_versions`, `dashboard_versions`, etc.).
2. Résout le graphe de dépendances (`--with-deps` : le dashboard embarque ses flows).
3. **Slotification** : chaque référence vers un device, une capability, un POI ou un objet hors bundle devient un slot typé. Le type est dérivé du schéma `pnex-core`. Le nom proposé vient du libellé local et reste éditable.
4. Chaque config de site ou secret (référence de coffre D110) devient un requirement.
5. **Échec bloquant** si une référence ne peut pas être convertie : l'export ne produit jamais un bundle partiellement portable.
6. Génère le `kit.toml` pré-rempli et un `README.md` squelette.

## 7 bis. Classification déclarative des champs (le vrai cœur de l'export)

L'export n'est **pas** une simple sérialisation. Un même objet mélange du contenu portable, du contexte local, des secrets et des données personnelles. Les notifications en sont le cas le plus piégeux (cf. tableau ci-dessous).

### Principe

Chaque champ de chaque schéma `pnex-core` porte une **classe de sensibilité**, déclarée une seule fois (attribut/derive Rust) :

| Classe | Sens | Traitement à l'export |
|---|---|---|
| `portable` | Logique, structure, contenu générique | Exporté tel quel |
| `param` | Valeur réglable (seuil, fréquence, throttling) | Exporté comme **paramètre** avec valeur par défaut, réglable à l'install |
| `local_ref` | Référence à un objet de l'instance | Converti en **slot** typé |
| `secret` | Credential, token, URL porteuse de token | Converti en **requirement**, valeur jamais exportée |
| `pii` | Destinataires, emails, téléphones, noms de personnes | Converti en slot vers users/groupes locaux, valeur jamais exportée |

- Un champ **sans classe** fait échouer la compilation du schéma : aucun nouveau champ ne peut échapper à la classification.
- L'export s'appuie **uniquement** sur ces annotations. Le scan heuristique de la CI (§9) reste un **filet de sécurité**, pas le mécanisme principal.
- Les **chaînes libres** (`portable` mais texte : templates, descriptions, labels) passent en plus par un lint dédié : détection de tokens, URLs, emails, IPs et noms d'org en dur.

### Cas des notifications

| Élément | Exemple | Classe → destin |
|---|---|---|
| Canal + credentials | SMTP, token bot, **URL de webhook** (l'URL est le secret) | `secret` → requirement `notification_channel{kind=email\|webhook\|…}` |
| Destinataires | emails, numéros, users | `pii` → slot `recipients` (users/groupes locaux) |
| Template de message | « Alerte {{device}} : {{value}} °C » | `portable` → exporté ; variables validées contre les ports/slots du flow ; lint anti-secrets/anti-org |
| Partials / templates partagés | en-tête commun réutilisé | `local_ref` si hors kit → slot ou dépendance ; embarqué si `--with-deps` |
| Seuils, fenêtres, throttling | > 35 °C, « N en T », max 1 notif / 10 min | `param` → valeur par défaut + réglage à l'install |
| Politique d'escalade / horaires | astreinte, plages silencieuses | Contexte local → requirement optionnel, jamais embarqué |

### Conséquences sur l'install

L'écran d'install (§11) comporte trois étapes distinctes : **Slots** (binding), **Requirements** (canaux, secrets, profils réseau), **Paramètres** (valeurs par défaut modifiables). Un kit dont une notification n'a pas de canal résolu s'installe, mais la notification est en statut « inactive » et c'est signalé.

## 8. Publication (bundle → PR)

**CLI** : `pnex kit publish ./mon-kit [--tap Pnex/hub]`

1. Authentification GitHub via **device flow** (pas de token à copier).
2. Fork du hub si nécessaire → branche `kit/<id>-<version>` → commit → PR.
3. Le template de PR est pré-rempli : description, checklist, captures.
4. Lint local identique à la CI, exécuté avant le push.

**UI** (phase ultérieure) : bouton « Partager » → OAuth GitHub App → même flux, pour les non-devs.

## 9. Validation (CI du hub)

Exécutée sur chaque PR. Une CI rouge bloque le merge.

| Check | Détail |
|---|---|
| **Schéma** | Chaque objet est validé contre `schemas/` (généré depuis `pnex-core`, versionné) |
| **Compat** | `pnex` range valide ; `schema_version` supportée par la plage déclarée |
| **Portabilité** | Rejet de tout UUID, IP, MAC, SSID, URL interne ou chaîne ressemblant à un secret (entropie + patterns) |
| **Cohérence** | Tout `$slot`, `$req`, `$local` utilisé est déclaré ; aucun slot déclaré n'est orphelin |
| **Scripts** | Lint JS / Starlark ; interdiction des lookups d'objets par nom (les données passent par les ports) ; scan de dépendances |
| **Install test** | Une instance PNEX éphémère (docker compose) installe le kit avec des bindings mock et vérifie le déploiement |
| **Rendu** | Génération automatique d'un screenshot du dashboard ou du synoptique, posté en commentaire de PR |
| **Méta** | `id` unique, version semver incrémentée, licence MIT, DCO signé |

Revue humaine ensuite (mainteneur), focalisée sur l'intérêt et la qualité, la mécanique étant déjà validée.

## 10. Catalogue et index

- Au merge sur `main`, la CI publie `index.json` (+ archives des kits avec checksum SHA-256) sur **GitHub Pages / Releases**.
- L'index contient : id, versions disponibles, compat, tags, slots, requirements, cover, checksum.
- Les instances PNEX le récupèrent périodiquement (avec cache, Valkey) ou à la demande, via l'egress guard (R8).

## 11. Installation (index → instance)

1. **Parcourir** : catalogue dans l'UI PNEX, filtré automatiquement sur la compat de l'instance.
2. **Prévisualiser** : README, cover, liste des slots et requirements.
3. **Binder** : écran de mapping slot → objet local.
   - suggestions automatiques par capability + unité + tags ;
   - sélecteurs pour les slots `many` ;
   - slots laissés non bindés autorisés : l'objet est créé en statut « non bindé ».
4. **Résoudre les requirements** : choix d'un profil réseau existant, création d'un secret dans le coffre, ou skip si `optional`.
5. **Migrer** si `schema_version` < version courante (migrations dans `pnex-core`).
6. **Créer** les objets comme nouvelles entités locales (nouvelle version de flow, etc.), **enregistrées mais non déployées**. Le déploiement reste une action explicite, cohérent avec la séparation enregistrer / déployer.
7. Traçabilité : chaque objet garde `origin = {tap, kit_id, version, checksum}` (provenance D184).

**CLI** : `pnex kit install meteo-station-c6@1.2.0 --bind temp_ext=<uuid>` pour l'automatisation.

## 12. Robustesse locale

| Événement | Comportement attendu |
|---|---|
| Renommage d'un device | Aucun impact : le binding est par UUID |
| Remplacement de carte | Rebind du slot vers le nouveau device ; flow et dashboard intacts |
| Device supprimé | Le slot passe en « non bindé », warning visible sur les objets concernés, pas de crash |
| Changement de Wi-Fi | Modification du profil réseau → rebuild/reprovisioning ; le kit n'est pas touché |
| Nouvelle version du kit | Notification « mise à jour dispo » via `origin` ; mise à jour = nouvelle version locale + réutilisation des bindings existants ; diff affiché avant |

## 13. Versionnement

- **Kit** : semver. Majeur = changement de slots/requirements (rebinding nécessaire).
- **Objets** : `schema_version` par type, migrations **montantes uniquement** dans `pnex-core`.
- **Plateforme** : range `pnex` obligatoire dans le manifeste.

## 14. Sécurité et confiance

- Scripts toujours exécutés dans les sandbox existantes (QuickJS via rquickjs, Starlark) : un kit n'obtient aucun privilège supplémentaire.
- Aucune I/O dans les scripts : SQL, HTTP, MQTT et WebSocket restent dans les nœuds natifs validés.
- Checksums vérifiés à l'install.
- Rôle : installer un kit = écriture, garde de rôle + test viewer → 403 (R2) ; org depuis le principal (R1).
- **Plus tard** : signature des bundles (cosign/sigstore), badge « vérifié » pour les kits revus par le mainteneur ou les fabricants partenaires.
- À l'install, l'UI affiche explicitement ce que le kit va créer et ce à quoi il accède.

## 15. Taps (hubs tiers et privés)

- Toute instance peut ajouter des sources (UI de l'org ; CLI `pnex tap add acme https://git.acme.local/pnex-hub` en équivalent).
- Même structure, même CI (réutilisable via une action GitHub/GitLab publiée par PNEX).
- Cas d'usage : hub interne d'une org industrielle, hub d'un fabricant de cartes, hub d'un fork communautaire.
- Le hub public est un tap par défaut, désactivable (air-gap).

## 16. Dépendances à la refacto ontologie (0.2.0) — raison du gel

L'implémentation attend la 0.2.0 parce que :

1. **Références par UUID partout** : toutes les références inter-objets doivent passer par UUID. Sans ça, la slotification n'est pas fiable.
2. **Typage des slots** : le type d'un slot devrait s'exprimer dans le vocabulaire de l'ontologie (capability, unité, type d'entité, relation POI/placement) plutôt que dans un système ad hoc.
3. **Schémas stables** : figer `schema_version` v1 sur les schémas post-refacto évite une première vague de migrations inutiles.
4. **Les makers d'abord** : le principe « un device doit fonctionner seul sans ontologie » doit rester vrai pour les kits. Un kit simple ne doit exiger que des slots par capability, sans modélisation ontologique.
5. **Classes de sensibilité** (§7 bis) : les annotations `portable` / `param` / `local_ref` / `secret` / `pii` doivent être posées sur les schémas `pnex-core` pendant la refacto, pas ajoutées après coup.

**À faire pendant le gel** : rien côté code. Garder ce PRD aligné avec les décisions de la 0.2.0 — en particulier, le **pack** D190 et le **kit** doivent être un seul format ou une relation explicite (§19, question 7).

## 17. Phasage (post-0.2.0)

| Phase | Contenu | Livrable |
|---|---|---|
| **P1** | Format de bundle, `kit.toml`, schémas générés depuis `pnex-core`, export (UI + CLI) | Export fiable et portable |
| **P2** | Repo `Pnex/hub`, CI de validation (sans install test), `pnex kit publish` | Premières PR communautaires |
| **P3** | `index.json`, UI catalogue + install + binding, `origin` | Install en un clic |
| **P4** | Install test éphémère et screenshots en CI, mises à jour de kits | Revue quasi automatique |
| **P5** | Bouton « Partager » UI, taps, signature | Ouverture aux non-devs et aux orgs |

## 18. Critères d'acceptation (v1 = P1 → P3)

- [ ] Un dashboard multi-devices exporté depuis une instance A s'installe sur une instance B vierge, se binde et fonctionne.
- [ ] Renommer ou remplacer un device bindé ne casse aucun objet installé.
- [ ] Aucun bundle mergé ne contient d'UUID, IP, SSID ou secret (vérifié par la CI).
- [ ] Export + publish réalisables en < 2 min par un dev.
- [ ] Un kit installé est enregistré non déployé ; le déploiement reste explicite.

## 19. Questions ouvertes

1. **Kits « hardware-first »** : faut-il un type d'objet `firmware_profile` distinct de la variante de carte, ou variante + capabilities suffisent-elles avec le firmware générique ? (Le firmware custom D87–D94 est un candidat naturel.)
2. **Dépendances entre kits** (un kit qui réutilise une fonction publiée par un autre kit) : supportées en v1 ou interdites ?
3. **Fonctions** : source seule (JS / Starlark, pas de binaire) — trancher si un jour WASM entre dans la stack.
4. **Modération** : faut-il des co-mainteneurs par catégorie (`CODEOWNERS`) quand le volume monte ?
5. **Identité** : le compte GitHub suffit-il comme identité d'auteur, ou faut-il un lien avec le compte Rauthy de l'instance ?
6. **Hébergement de l'index** : GitHub Pages seul, ou miroir (Codeberg / object storage) pour la résilience et la souveraineté ?
7. **Kit vs pack D190** : même format (un pack métier = un kit officiel) ou deux notions ?

## 20. Journal de relecture (2026-10-10)

Corrections apportées à la v0.1 à l'intégration :

1. **WASM / wasmtime, Python, Lua** : absents de la stack. Les fonctions
   custom sont en JS (rquickjs) et Starlark ; §5, §9, §14 et Q2–Q3 réécrits.
2. **Board profiles** : le catalogue de cartes vit dans le code
   (`pnex_core::catalog`, D121) ; l'objet partageable est la **variante de
   carte** utilisateur, pas le profil catalogue.
3. **UI = seule interface utilisateur** : la CLI reste pour les
   contributeurs et l'automatisation ; chaque geste utilisateur (export,
   install, taps) existe dans l'UI.
4. **Noms** : `flow_versions` (pas `flow_version`), org GitHub `Pnex`,
   `pnex-core` (pas `core`).
5. **§16** : numérotation 3 → 5 → 4 remise dans l'ordre.
6. Ajouts : R1/R2 à l'install, egress R8 pour l'index, secrets dans le
   coffre (D110), `origin` = provenance D184, modèles de page (`pages.md`)
   comme objets partageables, question 7 (kit vs pack D190).

## 21. Décisions tranchées (2026-10-10)

- **Kit vs pack D190 (Q7)** : **un seul format** — un pack métier est un
  kit officiel ; un seul manifeste, une seule installation. À refléter
  dans D190 pendant la 0.2.0.
- **Identité (Q5)** : le compte GitHub seul (PR + DCO), sans lien avec le
  compte Rauthy de l'instance.
- Restent ouvertes : Q1 (hardware-first), Q2 (dépendances entre kits),
  Q3 (WASM un jour), Q4 (modération), Q6 (hébergement de l'index).
