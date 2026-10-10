# PRD — PNEX Pages : documents collaboratifs reliés à l'ontologie

**Statut :** Draft v0.1 — 2026-10-10. Zéro code avant validation.
**Auteur :** Shan
**Périmètre :** nouvelle surface d'édition collaborative (texte riche, tableaux,
mesures, procédures/workflows, cartes, médias), dont chaque bloc peut désigner
n'importe quel objet PNEX.
**Version cible :** à trancher (§11, question 1) — dépend du noyau ontologique
de la 0.2.0 (`ontology.md`, D176–D191), et le gel des nouveaux piliers pendant
la 0.2.0 s'applique (`roadmap.md` P2.14). Entrée roadmap : P2.17, décision #21.
**Numérotation :** décisions notées **P1…P16** ici ; numéros D attribués à la
validation (D192+ déjà réservé par `media-vision-studio.md`).
**Renvois :** `ontology.md` (D176 types, D179 liens, D183 actions, D184
provenance, D185 requête, D186 explorateur, D188 permissions, D190 packs),
`media.md` (D21 bibliothèque média, MediaStore), `viz-bases.md` (`pnexMap`,
pont JS → Rust, modes live/édition D34), `geo-layers.md` (fond de carte de
l'org, couches), `doc-search.md` (index de recherche des documents),
`dashboard_editor` / D129 (liaisons de lecture `SourceRef`),
`horizontal-scaling.md` (D108 Valkey obligatoire), `ai-assistant.md` §9
(D142–D145), `security.md` (R1–R20, R11–R13 en particulier),
`editor-shell.md`.

---

## 1. Constat

PNEX sait capter, stocker, calculer, afficher et agir. Il ne sait pas
**écrire**. Aujourd'hui, ce qui entoure les données vit hors de PNEX :

- la procédure de maintenance d'une pompe est un Word sur un partage réseau ;
- le relevé de mesures d'une recette (mise en service, réception) est un
  Excel envoyé par mail ;
- le compte rendu d'incident cite un capteur par son nom, sans lien, et la
  courbe est une capture d'écran figée ;
- la note d'implantation d'un site est un PDF avec une carte exportée.

Conséquences : basculement d'outils (contraire à l'exigence « une seule
surface »), aucune traçabilité entre le texte et les faits, des chiffres qui
divergent, et aucune version fiable.

L'ontologie 0.2.0 donne à chaque chose un identifiant et un type. Il manque
l'endroit où les humains **raisonnent et travaillent ensemble** sur ces
objets.

## 2. Vision

> **Une page PNEX est un document vivant : on y écrit à plusieurs, en temps
> réel, et chaque mention d'un objet est un lien vers le vrai objet, pas une
> copie de son nom.**

Une page mêle librement :

| Bloc | Rôle |
|---|---|
| Texte riche | titres, listes, citations, code, callouts, liens |
| Mention `@` | référence typée à n'importe quel objet (device, pompe, flow, dashboard, plage, média, POI…) avec puce vivante |
| Tableau local | cellules libres + formules, type tableur |
| Vue de collection | tableau / kanban dont les lignes **sont** des objets de l'ontologie, éditables en place |
| Mesure | valeur saisie avec unité, tolérance, opérateur, horodatage ; ou valeur lue sur un objet |
| Procédure / checklist | étapes ordonnées, validation, signature, déclenchement d'action |
| Carte | morceau de carte MapLibre (vue, couches, objets, géozones), live ou figé |
| Graphe | série d'un objet, live ou figée à une date |
| Média | image, vidéo, PDF, panorama 360, issus de la bibliothèque média |
| Widget | n'importe quel widget de dashboard existant |

Promesses : **collaboratif temps réel**, **versionné**, **importable**,
**hors de toute dépendance SaaS**, tenant sur un Raspberry Pi.

## 3. Utilisateurs et cas d'usage

| Persona | Cas d'usage vedette |
|---|---|
| Technicien maintenance | Ouvre la procédure « Remplacement garniture P-12 » sur tablette, coche les étapes, saisit les mesures (couple, pression d'essai), signe ; la mesure hors tolérance bloque la clôture |
| Responsable méthodes | Rédige la procédure type pour le type « Pompe » ; elle s'instancie sur chaque pompe (école dashboard de type D187) |
| Ingénieur mise en service | Relevé de recette : vue de collection des 40 capteurs d'une ligne, colonne « valeur lue » + colonne « valeur de référence » + écart calculé |
| Chef de projet / analyste | Compte rendu d'incident : texte + courbe figée à la plage de l'incident + carte du site + mentions des équipements ; la page apparaît dans l'onglet « Pages » de chaque objet mentionné |
| Maker | Journal de projet : notes, photos, schéma, mention de ses devices avec leur dernière valeur |
| Analyste média (Repère) | Note d'enquête : mentions de personnalités, plages d'émission, segments de transcription sourcés |

## 4. Non-objectifs (V1)

- La parité Notion, Google Docs ou Excel. On vise l'utile pour l'opérationnel.
- Un tableur de calcul lourd (macros, VBA, tableaux croisés, solveurs). Le
  calcul lourd reste dans les flows et les fonctions (`ontology.md` §3).
- Du code exécutable dans une page (pas de notebook).
- L'édition WYSIWYG de `.docx` en fidélité Word. On **importe** le contenu,
  on ne reproduit pas la mise en page.
- Les ACL par page et le partage hors org (alignés sur D188 : décision
  ultérieure, modèle anticipé).
- Les iframes arbitraires et le HTML brut (R11–R13).
- Publier une page sur le web public.

## 5. Décisions proposées

### P1 — La page est un type système de l'ontologie

- `doc` est déclaré dans le registre de types (D176), `system = true`.
  Elle a donc des propriétés, des labels, un containment (dossier, ou sous
  un objet : « pages de la pompe P-12 »), des liens et une provenance.
- Une page peut être **rattachée** à un objet (containment) ou être
  **libre** dans un espace de pages de l'org.
- Les pages sont listées par l'explorateur (D186) et par un onglet
  **Pages** sur chaque page objet.

### P2 — Modèle d'édition : CRDT côté serveur et côté client

- Le contenu d'une page est un **document CRDT**. Candidat : **Loro**
  (Rust, MIT) — texte riche, listes, maps, arbre déplaçable, et
  historique natif (checkout d'une version, diff) qui sert directement
  P5. Alternative : **yrs** (port Rust de Yjs, MIT).
- Le **serveur Loco détient une réplique** de chaque page ouverte : il
  valide, persiste, compacte et sert les snapshots. Le client n'est
  jamais la source de vérité.
- Pourquoi un CRDT plutôt qu'OT ou « dernier écrit gagne » : multi-pods
  sans serveur d'ordonnancement unique, édition hors ligne sur mobile
  avec fusion au retour, et historique gratuit.
- À trancher au spike L0 : Loro vs yrs (critères : mémoire sur Pi,
  liaison éditeur, maturité de l'historique, taille du bundle wasm).

### P3 — Éditeur : ProseMirror en bundle IIFE, piloté par Dioxus

- Dioxus n'a pas d'éditeur de texte riche viable ; en écrire un est hors
  de portée. On suit l'école `pnexMap` (`viz-bases.md`) : bundle esbuild
  IIFE `js/doc.js` → `window.pnexDoc`, pont `Closure::wrap`, stub natif,
  échec → badge, jamais panic.
- Contenu : **ProseMirror** (MIT, headless, aucun service distant) + la
  liaison CRDT (`loro-prosemirror` ou `y-prosemirror` selon P2).
  Tiptap écarté : cœur MIT mais extensions de collaboration et
  historique couplées à un cloud payant.
- Les **blocs PNEX** (mention, mesure, carte, graphe, vue de collection,
  widget, média) sont des **nœuds ProseMirror atomiques** dont le rendu
  est délégué à Dioxus (portails) : l'identité visuelle reste celle de
  PNEX, le JS ne gère que le texte et la sélection.
- Le schéma ProseMirror (types de blocs, attributs) est **généré depuis
  `pnex-core`** : une seule définition pour l'éditeur, la validation
  serveur, l'import/export et l'assistant.

### P4 — Transport temps réel : WebSocket par page, fan-out Valkey

- Nouvelle route `/ws/doc/{id}`, authentifiée comme l'UI (session Rauthy),
  org depuis le principal (R1), viewer → lecture seule, écriture refusée
  côté serveur (R2).
- Multi-pods : chaque pod qui sert une page s'abonne à
  `pnex:{db}:doc:{id}` (Valkey pub/sub) ; une mise à jour reçue est
  appliquée à la réplique locale, persistée une fois (pod propriétaire
  par bail Valkey, école D108), diffusée aux autres.
- **Présence** (curseurs, sélection, « qui regarde ») : Valkey uniquement,
  à expiration, jamais en base.
- Persistance : journal append-only des mises à jour + **compaction**
  périodique en snapshot (seuil de taille ou d'inactivité).
- Mobile hors ligne : la mise à jour locale est rejouée à la reconnexion,
  la fusion est celle du CRDT.

### P5 — Versionnement : continu + versions nommées

- **Historique continu** : chaque session d'édition est rejouable
  (« voir la page au 3 mars, 14 h »), grâce au CRDT.
- **Versions nommées** (append-only, école D18) : « v1 — validée par X ».
  Une version nommée est figée, signable, citable par URL, exportable.
- **Diff** entre deux versions (texte + blocs ajoutés/retirés/modifiés).
- **Restaurer** = créer une nouvelle version au contenu ancien ; rien
  n'est jamais effacé.
- **Rétention** de l'historique continu réglable par org (D72) ; les
  versions nommées ne sont jamais purgées automatiquement.
- Branches / mode suggestion : hors V1 (§10).

### P6 — Mentions : des liens de l'ontologie, pas du texte

- `@` ouvre un sélecteur alimenté par la recherche globale (D69) et la
  requête ontologique (D185), filtrable par type.
- Une mention stocke un `ResourceRef` (+ propriété optionnelle :
  `@P-12.température`). Le libellé affiché est **résolu au rendu** :
  renommer l'objet met à jour toutes les pages.
- **Puce vivante** : icône du type, titre, état (en ligne, en alarme,
  déployé…), dernière valeur pour une propriété `series`. Survol =
  aperçu ; clic = page objet.
- Chaque mention crée un **lien système `mentions`** (D179) doc → objet,
  avec `source_ref = doc:<id>@<version>` (D184). D'où les **rétroliens** :
  l'onglet Pages d'un objet liste toutes les pages qui le citent.
- Objet supprimé ou inaccessible → puce grisée « objet introuvable », la
  page reste lisible.

### P7 — Deux sortes de tableaux, à ne pas confondre

**(a) Vue de collection** — la pièce maîtresse :

- Les lignes **sont** des objets (requête D185 : type, filtres,
  containment) ; les colonnes sont leurs propriétés (D178), y compris
  temporelles (dernière valeur, agrégat sur fenêtre ou sur plage D182).
- Édition en place → écriture via le **service ontologique partagé**
  (mêmes validations, `expected_version`, 409), provenance
  `doc:<id>`. Aucune donnée métier n'est stockée dans la page : la page
  ne stocke que la requête et la mise en forme.
- Affichages : tableau, kanban (regroupé par une propriété `enum`),
  galerie ; plus tard calendrier / chronologie.
- Colonnes **locales** autorisées (commentaire de recette, coche) : elles
  vivent dans la page, pas dans l'objet, et sont visuellement distinguées.

**(b) Tableau local (type tableur)** :

- Grille libre avec formules, stockée dans la page.
- Moteur candidat : **IronCalc** (Rust, MIT/Apache-2.0, compile en
  wasm32, formules type Excel, import/export `.xlsx`). Même moteur côté
  client (recalcul instantané) et serveur (validation, export) :
  déterministe des deux côtés.
- Collaboration au niveau cellule (map CRDT : dernier écrit gagne par
  cellule), recalcul local.
- **Fonctions PNEX** dans les formules, en lecture seule :
  `PNEX.LAST(@P-12, "température")`, `PNEX.AVG(@P-12, "débit", "24h")`,
  `PNEX.RANGE(@poste-matin, …)`. Résolues côté serveur via D185, mises en
  cache, rafraîchies à la demande ou à intervalle (pas de streaming par
  cellule). Une formule ne peut **jamais** écrire.
- Promotion : un tableau local peut être converti en **type d'objet**
  (colonnes → propriétés, lignes → objets) — c'est la voie naturelle
  « mon Excel devient de la donnée PNEX ».

### P8 — Mesures : valeurs avec unité, tolérance et provenance

- Bloc `measure` : libellé, valeur, **unité** (réutilise
  `unit_conversions`), tolérance (min/max ou nominal ± écart), opérateur,
  horodatage, instrument (mention optionnelle).
- Deux modes : **saisie** (main humaine) ou **lecture** (valeur d'une
  propriété d'objet, capturée à l'instant de la validation, avec sa
  provenance device/flow).
- Hors tolérance → badge rouge + compte dans l'en-tête de page.
- Option « publier la mesure » : écrit un point dans une série de
  l'objet (`source_ref = manual:<user>` via `doc:<id>`). La mesure
  manuelle devient un fait historisé comme les autres.

### P9 — Procédures et workflow de page

Deux niveaux, distincts :

- **Workflow de la page** (cycle de vie) : brouillon → en relecture →
  validée → obsolète. Transitions par rôle, relecteurs désignés,
  validation = création d'une version nommée signée (P5). Défini par
  type de page dans le schéma (D176), donc personnalisable par pack.
- **Procédure** (bloc) : étapes ordonnées, chacune avec checklist,
  mesures (P8), photo obligatoire optionnelle, validation par
  l'exécutant. Une **instance** de procédure = une exécution datée,
  attachée à l'objet, figée à la clôture.
- Une étape peut **proposer une action** déclarée (D183) : bouton
  « Mettre P-12 en maintenance ». L'action passe par son flow déployé,
  avec confirmation si `confirm: true`, et est journalisée (`actions`).
  La page ne commande jamais un device elle-même (D123, D128).
- Lien GMAO : la procédure instanciée est la brique « bon de travail »
  attendue par le chantier GMAO ; ne pas la redéfinir là-bas.

### P10 — Carte : un morceau de MapLibre, pas une capture

- Bloc `map` réutilisant `pnexMap` : vue (centre, zoom, cap,
  inclinaison), fond de carte de l'org (fournisseur `basemap` par défaut,
  `geo-layers.md` L24–L26), couches sélectionnées, objets/POI/géozones
  mentionnés, annotations simples (point, ligne, polygone, texte).
- **Live** (positions et états courants) ou **figé à une date** (états à
  T, via liens temporels D179) — indispensable pour un compte rendu.
- Les formes dessinées sont des données de la page ; « promouvoir en
  géozone » crée l'objet correspondant.
- Export : image statique générée côté serveur pour PDF/Markdown.

### P11 — Médias et import

- **Médias** : glisser-déposer → upload dans la bibliothèque média (D21,
  MediaStore, octets jamais en base), le bloc référence l'`asset`.
  Image, vidéo (lecteur existant), PDF (aperçu), panorama 360 (viewer
  existant). Un média supprimé de la bibliothèque laisse un bloc
  « média introuvable ».
- **Import** (chaque format est rangé là où il se prête) :

| Source | Devient |
|---|---|
| `.md` | texte riche (titres, listes, tableaux, code) |
| `.docx` | texte riche + images vers la bibliothèque ; mise en page abandonnée |
| `.csv`, `.xlsx` | tableau local (P7b) ou, au choix, import en objets d'un type |
| `.geojson`, `.gpx`, `.kml` | bloc carte + formes (ou objets/géozones, ou layer `geo-layers.md`) |
| images, vidéos, `.pdf` | bloc média |
| page HTML collée | texte riche nettoyé (aucun HTML brut conservé) |

- Import = job de fond (queue Postgres Loco existante), provenance
  `import:<job>`. Les extracteurs `.docx`/`.md`/`.csv`/`.xlsx` sont ceux de
  `doc-search.md` (trait `Extractor` de `pnex-core`), pas une seconde
  implémentation.
- **Export** : Markdown (avec mentions en liens PNEX), HTML autonome, PDF
  (lot ultérieur). Une version nommée s'exporte telle qu'elle était.

### P12 — Commentaires

- Fils de commentaires ancrés sur une plage de texte ou un bloc (ancres
  CRDT : survivent aux éditions). Mentions de personnes → notification
  (moteur de notifications existant).
- Résolu / rouvert ; les commentaires résolus restent consultables.
- Les commentaires ne font pas partie du contenu versionné (P5), mais
  sont rattachés à la version où ils ont été posés.

### P13 — Modèles et packs

- **Modèle de page** : page figée réutilisable, éventuellement **par
  type d'objet** (« procédure type Pompe » s'instancie sur chaque pompe,
  les mentions `@this` se résolvent vers l'objet hôte).
- Les modèles voyagent dans les **packs** (D190) : le pack
  « Maintenance augmentée » apporte procédure de remplacement, rapport
  d'intervention, relevé de recette.
- Export/import YAML + contenu (as-code) sur la même source de vérité.

### P14 — Assistant IA

Conformément à `ai-assistant.md` §9 (toute fonctionnalité lui est livrée) :

- Lecture : contenu des pages, versions, mentions, commentaires (dans
  l'org, R1).
- Écriture **via le service partagé avec l'UI** : rédiger depuis un
  modèle, résumer, reformuler une sélection, remplir une vue de
  collection, générer un compte rendu à partir d'une plage + des objets
  mentionnés. Les éditions de l'assistant arrivent comme celles d'un
  collaborateur (présence « Assistant », provenance
  `assistant:<conversation>`), annulables.
- Interdits inchangés : aucune action physique, aucune exécution
  d'action D183 (proposition uniquement), pas de validation ni de
  signature de page, pas de modification d'une version nommée.
- Fiches `assistant-kb` (page Pages, chaque bloc, codes d'erreur) dans le
  même commit que le bloc ; test « registre == ensemble autorisé » mis à
  jour pour chaque nouvel outil.

### P15 — Sécurité

- Frontière = org (R1) ; viewer en lecture seule vérifié sur le WS **et**
  sur l'API (R2), test viewer → 403 par route d'écriture.
- **Aucun HTML utilisateur sur l'origine de l'app** (R11–R13) : le
  contenu est un arbre typé validé contre le schéma P3 côté serveur ;
  toute mise à jour CRDT non conforme est rejetée. Liens externes en
  `rel=noopener noreferrer`, schémas d'URL en liste blanche.
- Une mention ne donne **aucun accès** : la puce est résolue avec les
  droits du lecteur ; objet non visible → « objet introuvable », sans
  fuite de titre (préparation des ACL par objet, D188).
- Taille max de page, de mise à jour et de pièce jointe ; débit de mises
  à jour par connexion borné.
- Aucun secret ni identifiant interne dans les DTO de lecture (R4, R16).

### P16 — i18n et accessibilité

- Tout texte d'interface via `t!`, clés dans les deux `.ftl`. Le
  **contenu** des pages est du texte libre, non traduit (exception
  verbatim documentée, comme les notes média).
- Navigation clavier complète de l'éditeur, palette `/` pour insérer un
  bloc, rendu mobile utilisable en lecture et en exécution de procédure
  (tablette terrain en priorité).

## 6. Modèle de données (récapitulatif)

| Table / stream | Rôle |
|---|---|
| `docs` | une ligne par page : `org_id`, `title`, `doc_type`, `workflow_state`, `template_id`, `latest_snapshot_id`, timestamps (type système `doc`, P1) |
| `doc_updates` | journal append-only des mises à jour CRDT (binaire), compacté (P4) |
| `doc_snapshots` | snapshots compactés (binaire, en base ou MediaStore au-delà d'un seuil) |
| `doc_versions` | versions nommées, append-only, signatures (P5, P9) |
| `doc_comments` | fils de commentaires ancrés (P12) |
| `doc_procedure_runs` | instances de procédures exécutées, figées à la clôture (P9) |
| `doc_templates` | modèles, éventuellement par type d'objet (P13) |
| `resource_edges` (lien système `mentions`) | rétroliens doc → objet (P6) |
| `media_assets` (existante) | octets des médias embarqués (P11) |
| Valkey `pnex:{db}:doc:*` | fan-out, bail de pod propriétaire, présence (P4) |
| O2 `object_changes` | provenance des écritures faites depuis une page (D184) |
| Index plein texte (`tsvector`) | recherche globale D69 étendue aux pages ; même stratégie que `doc-search.md` (question 6 de ce PRD) |

## 7. API (additive)

- `GET/POST /api/v1/docs`, `GET/PATCH/DELETE /api/v1/docs/{id}`
- `GET /api/v1/docs/{id}/snapshot?at=<ts|version>`
- `GET/POST /api/v1/docs/{id}/versions`, `GET …/versions/{a}/diff/{b}`,
  `POST …/versions/{v}/restore`
- `POST /api/v1/docs/{id}/transition` (workflow P9)
- `GET/POST /api/v1/docs/{id}/comments`
- `POST /api/v1/docs/import` (job), `GET /api/v1/docs/{id}/export?format=md|html`
- `POST /api/v1/docs/formula/resolve` (fonctions PNEX des tableaux, P7b)
- `GET /api/v1/ontology/objects/{ref}/docs` (rétroliens)
- `WS /ws/doc/{id}` (synchronisation + présence)

Erreurs = codes machine dans `pnex_core::err_codes::ALL` + clés `err-*`
dans les deux `.ftl`.

## 8. Lots

| Lot | Contenu | Sortie vérifiable |
|---|---|---|
| **L0 — Spike** | Loro vs yrs ; ProseMirror en IIFE dans Dioxus (web + natif wry) ; WS + fan-out Valkey sur 2 pods ; mesure mémoire Pi 4 avec 10 pages ouvertes ; IronCalc en wasm32 (taille, perf) | note de spike + décision P2/P3/P7b |
| **L1 — Page solo** | type `doc`, éditeur texte riche, palette `/`, snapshots, versions nommées, diff, restauration, import/export Markdown | une page créée, versionnée, restaurée, exportée |
| **L2 — Collaboration** | WS temps réel, présence, multi-pods, hors ligne mobile, commentaires | 3 navigateurs + 1 mobile hors ligne sur 2 pods convergent ; E2E |
| **L3 — Objets** | mentions `@`, puces vivantes, lien `mentions`, rétroliens, onglet Pages, blocs graphe et widget (live / figé) | renommer un device met à jour la page ; rétrolien visible |
| **L4 — Médias & carte** | glisser-déposer média, blocs image/vidéo/PDF/360, bloc carte live/figé, import `.docx`, `.geojson`/`.gpx`/`.kml` | compte rendu d'incident complet sans quitter PNEX |
| **L5 — Tableaux** | vues de collection (tableau, kanban) éditables ; puis tableau local IronCalc, fonctions PNEX, import `.csv`/`.xlsx`, promotion en type | relevé de recette de 40 capteurs avec écarts calculés |
| **L6 — Procédures & workflow** | cycle de vie de page, relecture, signature ; bloc procédure, mesures avec tolérance, instances, actions D183 | procédure exécutée sur tablette, mesure hors tolérance bloquante |
| **L7 — Modèles, packs, assistant** | modèles par type, intégration packs D190, outils assistant complets, export PDF | pack Maintenance livré avec ses 3 modèles |

À chaque lot : clés i18n, codes d'erreur, revue de sécurité §6, fiches
assistant — conditions de « fini », pas des tâches à part.

## 9. Critères de succès

- Le compte rendu d'incident, la procédure et le relevé de recette se font
  **sans quitter PNEX** (zéro Word/Excel en pièce jointe sur le pack
  Maintenance).
- Convergence garantie : 0 divergence sur un test de fuzz (édition
  concurrente aléatoire, 5 clients, 2 pods, coupures réseau).
- Latence de propagation d'une frappe < 150 ms en LAN.
- Pi 4 (4 Go) : 5 éditeurs simultanés sur une page de 20 pages A4 sans
  dépasser la cible mémoire de la stack (< 1 Go au repos, budget pages à
  fixer au L0).
- Toute valeur chiffrée d'une page répond à « d'où vient ce chiffre ».

## 10. Hors V1 (explicitement repoussé)

- Mode suggestion / branches de page avec fusion.
- ACL par page, partage hors org, lien public (suit D188).
- Base de connaissances publique, wiki inter-orgs.
- Vues calendrier / chronologie / Gantt des collections.
- Diagrammes dessinés dans la page (renvoi vers l'éditeur de synoptiques).
- Export `.docx` fidèle.

## 11. Questions à trancher (appartiennent au produit)

1. **Version cible** : après la 0.2.0 (respect du gel), ou en faire la
   « surface de travail » de la 0.2.0 ? Option intermédiaire : L1–L2
   (indépendants de l'ontologie) en exception, L3+ après la 0.2.0.
2. **Conflit avec `ontology.md` §3** (« pas de notebook ou workbook ») :
   le tableau local (P7b) est-il acceptable, puisqu'il ne calcule qu'en
   lecture et ne contient pas de code ? Proposition : oui, avec la
   frontière « formule = lecture, jamais d'écriture ni de code ».
3. **Loro ou yrs** (P2) — décision au spike L0.
4. **Pages privées** (brouillon visible de son seul auteur) : exige une
   ACL minimale par objet, donc toucher D188 plus tôt que prévu.
5. **Mesure publiée** (P8) : la saisie humaine a-t-elle le droit d'écrire
   dans une série alimentée par un device, ou dans une série dédiée
   `*_manual` uniquement ?
6. **Signature** des versions validées : simple trace d'identité (qui,
   quand) en V1, ou signature cryptographique (exigence défense/qualité
   pharma) ?
7. **Nom** de la fonctionnalité dans l'UI : « Pages », « Carnets »,
   « Documents » ? (« Documents » est déjà pris par l'onglet de
   `doc-search.md` F14 : à éviter ou à fusionner.)

## 12. Journal de relecture (2026-10-10)

Corrections apportées à la v0.1 à l'intégration :

1. **`NodeDoc` des blocs** retiré (P14) : `NodeDoc` ne documente que les
   nœuds de flow (`node_docs.rs`, garde `every_kind_is_documented`) ; un
   bloc de page se documente par une fiche `assistant-kb`.
2. **Fond de carte du bloc carte** rattaché aux fournisseurs `basemap` de
   `geo-layers.md` (L24–L26), seule source du fond par org.
3. **Extracteurs d'import** (P11) mutualisés avec `doc-search.md` : un seul
   trait `Extractor` dans `pnex-core` pour `.docx`, `.md`, `.csv`, `.xlsx`.
4. **Collision de noms** signalée : la table `docs` de ce PRD et la table
   `document` du PRD de recherche ; résolu côté `doc-search.md` (index
   rattaché aux versions de média, pas de table `document`). L'onglet
   « Documents » (`doc-search.md` F14) contraint la question 7.
5. **Q6 du PRD de recherche** (« les pages passent-elles par le même
   index ? ») : recommandé oui — même `tsvector`/trigrammes, donc la
   recherche globale D69 (aujourd'hui en `LIKE`) gagne un vrai index
   plein texte pour les deux chantiers.

## 13. Décisions tranchées (2026-10-10)

- **Version cible (Q1)** : **L1–L2 en exception au gel** (page solo +
  collaboration, indépendants de l'ontologie) ; L3+ après la 0.2.0.
- **Tableau local (Q2)** : accepté, frontière « formule = lecture, jamais
  d'écriture ni de code » ; `ontology.md` §3 amendé en conséquence.
- **Pages privées (Q4)** : non en V1 ; toute page est visible de l'org,
  les ACL suivent D188.
- **Mesure publiée (Q5)** : uniquement dans une série dédiée `*_manual`
  de l'objet, jamais dans une série alimentée par un device.
- **Signature (Q6)** : trace d'identité en V1 (qui, quand, version
  figée) ; signature cryptographique renvoyée aux profils de sécurité
  (`security-tiers.md`, V2+).
- **Nom (Q7)** : « **Pages** » ; « Documents » reste l'onglet des fichiers
  indexés (`doc-search.md` F14).
- Reste ouverte : Loro vs yrs (Q3), au spike L0.
