# Analyse : SQL en lecture, règles avec rejeu, blocs vivants (OP5, D825–D832)

| | |
|---|---|
| Statut | **Choix produit tranchés par Shan le 2026-11-08** (§2). Rien de codé |
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
