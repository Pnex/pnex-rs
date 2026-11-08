# Analyse : SQL en lecture, règles avec rejeu, blocs vivants (OP5, D825–D832)

| | |
|---|---|
| Statut | **Choix produit tranchés par Shan le 2026-11-08** (§2). **AN1 et AN2 codés le 2026-11-08** (§6) ; AN3 à faire |
| Déclencheur | Ordre OP1 → OP2 → OP3, puis OP5 (`operational-platform.md` §2) ; agrégats déjà livrés (`object-aggregates.md`) |
| Repose sur | `operational-platform.md` (D557 : lecture seule, droits de l'appelant), `data-pipelines.md` (worker D571–D574), `governance.md` (marquages D801), `crm.md` (masque D443), `ontology.md` (D185 requête), `pages` |
| Numérotation | **D825…D832**, lots **AN1…AN3** (AN4, analyse sans code, plus tard) |

## 1. Le besoin en une phrase

Poser une question en SQL à ses tables et à ses objets sans rien écrire, la surveiller par une règle
qu'on peut rejouer sur le passé, et en mettre la réponse vivante dans une page.

## 2. Choix de Shan (2026-11-08)

| Question | Réponse |
|---|---|
| Premier morceau | SQL en lecture |
| Où s'exécute une requête | Le worker DataFusion, en file |
| Ce que lit le SQL | Les tables **et** les objets de l'ontologie |
| Suite | Règles avec rejeu, puis blocs vivants ; l'analyse sans code attend |
| Portée des règles | Objets et séries |
| Bloc vivant dans un PDF | Valeur recalculée à l'export, datée |

## 3. Décisions

### D825 — Une requête est un travail du worker, sous les droits de l'appelant

`POST /analysis/queries` (`sql`) crée une requête en file ; `GET /analysis/queries/{id}` rend son
état puis son résultat. Le serveur fixe tout ce que la requête peut lire **au moment de l'envoi** :

- **tables** : celles de l'org que l'appelant lit, sauf les tables marquées pour lesquelles il n'est
  pas habilité, que la requête cite par leur nom et qui la font échouer en `marking-required` ;
- **objets** : chaque `objects.<type>` cité dans le texte devient une table figée des objets de ce
  type **tels que l'appelant les voit** (masque D443 et marquages D801 appliqués, objets cachés
  absents), écrite en Parquet dans un préfixe de la requête, effacée après.

Le worker n'a ni l'ontologie ni les droits : il lit seulement ce que le serveur lui a donné.

### D826 — Lecture seule, bornée

Mêmes garde-fous que les transformations (D572) : une seule instruction, `SELECT` seulement, liste
blanche de fonctions, pas de fichier ni de réseau, pool mémoire plafonné. En plus : 60 s, 1 000
lignes rendues (`truncated` au-delà), 10 Mo de résultat, une requête en cours ou en file par
personne (`analysis-busy`). Le résultat reste 24 h.

### D827 — Les requêtes passent avant les pipelines

Le worker prend d'abord une requête en file, puis un run de pipeline. Un run déjà commencé n'est pas
interrompu : sous une longue transformation, une requête attend. Un worker dédié aux requêtes
(`PNEX_WORKER_QUEUES=analysis`) se déploie à côté quand cette attente gêne.

### D828 — Une lecture marquée se trace

Une requête qui lit une table marquée (son auteur étant habilité) laisse une ligne
`governance_reads` avec les marquages et les finalités, comme un échantillon (D803).

### D829 — Règles : une condition, une alerte ou une proposition (AN2)

Une règle porte sur un type d'objet (filtres de la requête D185 sur ses propriétés) ou sur une série
(seuil, durée). Quand elle devient vraie : une notification ou une proposition, jamais une écriture
(D557). Elle s'évalue sous les droits de son auteur, revérifiés à chaque passage.

### D830 — Rejeu

Une règle se rejoue sur une période passée : l'historique des changements (objets) et les séries
(O2) donnent les instants où elle se serait déclenchée, sans rien envoyer. Le rejeu est un aperçu.

### D831 — Bloc vivant (AN3)

Un bloc de page porte une requête SQL ou un agrégat (D579) ; il se calcule à l'affichage, sous les
droits du lecteur (pas de l'auteur de la page) ; un lecteur sans droit voit le bloc vide avec la
raison.

### D832 — Export PDF

Un bloc est recalculé à l'export et porte « au <date> ».

## 4. Sécurité

- R1 : org de la requête = org du principal ; le worker ne lit que les clés que le serveur a listées.
- R2 : `analysis:run` (Membre) ; tables et objets filtrés à l'envoi ; marquages vérifiés à l'envoi.
- Le texte SQL est hostile : mêmes défenses que les transformations, plus le délai et le plafond.
- Le résultat est lisible par son seul auteur (404 pour les autres).

## 5. Lots

| Lot | Contenu |
|---|---|
| **AN1** | D825–D828 : requêtes SQL en lecture, objets figés, priorité, trace ; API, puis un éditeur simple dans `/pipelines` |
| **AN2** | D829–D830 : règles et rejeu |
| **AN3** | D831–D832 : blocs vivants dans les pages |
| AN4 | Analyse sans code (tableau croisé), plus tard |

## 6. Avancement

**AN1 (2026-11-08)** : migration 000131 (`analysis_queries`, droits du rôle restreint du worker),
`pnex_core::analysis`, `services/analysis.rs` (envoi : tables lisibles et refusées, objets figés en
Parquet sous le masque de l'auteur ; lecture ; purge à un jour dans la boucle horaire),
`controllers/analysis.rs` (`analysis_queries_create`, `analysis_queries_get`, `analysis:run` Membre,
exposés au MCP), module `analysis.rs` du worker (pris avant les runs, tables enregistrées seulement si
autorisées, schéma `objects`, 60 s, 1 000 lignes, 10 Mo ; une requête bloquée plus de 5 min échoue
d'elle-même). Onglet « Requête SQL » de `/pipelines` (Ctrl+Entrée, suivi de l'état, résultat en
tableau, raison du moteur affichée telle quelle sous le message traduit). Tests : `tests/analysis.rs`
(backend), `a_query_reads_only_what_the_server_fixed` (worker, vraie base et vrai stockage),
`e2e/tests/analysis.spec.ts` avec un vrai worker, deux fois vert.

Écarts à relire : la journalisation D828 se fait **à l'envoi**, pour les tables marquées dont le nom
apparaît dans le texte (une correspondance de mot, pas l'analyse du moteur) ; un objet figé d'une
requête jamais exécutée reste jusqu'à la purge du lendemain (`shortcut:` dans le code). `break-ui` :
le message d'une requête refusée disait « pas un seul SELECT » pour une colonne inconnue ; reformulé
en « le moteur a refusé : sa raison suit ».

**AN2 (2026-11-08)** : migration 000132 (`rules`, canal de notification optionnel, nom unique par
org), `pnex_core::rules` (condition objets ou série, filtres D185, évaluation pure des événements),
`services/rules.rs` (CRUD à `expected_version`, une condition changée remet l'état à zéro ; passage
toutes les 5 min en singleton `rules-pass`, sous le masque de l'auteur recalculé à chaque passage ;
objets : le premier passage note les objets qui correspondent sans rien envoyer, les suivants
notifient les nouveaux ; série : notifie quand le seuil tient la durée demandée ; envoi sur le canal
choisi, sinon notification dans l'app ; rejeu sur 24 h, 7 j ou 30 j sans rien envoyer),
`controllers/rules.rs` (`rules:read` Lecteur, `rules:write` et `rules:delete` Membre ; lecture et
rejeu exposés au MCP, écriture et suppression exclues). Page `/rules` sous Automatisation (tableau +
pager, formulaire, rejeu avec liens vers les objets), fiche `assistant-kb/rules.md`, ligne
« Règle » au lexique. Tests : `tests/rules.rs`, tests du cœur, `e2e/tests/rules.spec.ts`.

Écarts à relire : D829 prévoit « une notification **ou une proposition** » ; seule la notification
est livrée (choix de Shan pour AN2), la proposition attend OP9. `break-ui` : la colonne « Surveille »
montrait la clé du type au lieu de son nom, et l'état disait « fausse au dernier passage » pour une
règle jamais évaluée ; corrigés (nom du type, « Déclenchée / Non déclenchée », pluriel des filtres).
