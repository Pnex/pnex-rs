# Gouvernance des données : marquages, habilitations, finalité (OP3, D798–D806)

| | |
|---|---|
| Statut | **Choix produit tranchés par Shan le 2026-11-08** (§2). **G1 et G2 codés le 2026-11-08** (§7) |
| Déclencheur | Ordre OP1 → OP2 → OP3 (`operational-platform.md` §2) ; D715 : aucun client réel avant OP3 |
| Repose sur | `iam.md` (D285–D299, I4, I5), `crm.md` (D442 classification, D443 masquage, D446 effacement), `data-pipelines.md` (D569–D578, lignage), `connectors.md` |
| Numérotation | **D798…D806**, lots **G1…G7** |
| Hors OP3 | SAML (à la première demande client), I4 général (filtre des ~90 listes par portée d'attribution), I5 (ontologie fine), qui suivent |

## 1. Le besoin en une phrase

Qu'une donnée sensible (RH, santé, un client) ne soit lue que par les personnes habilitées, pour une
finalité déclarée, avec une trace de chaque lecture, y compris dans les tables qui en dérivent.

## 2. Choix de Shan (2026-11-08)

| Question | Réponse |
|---|---|
| Modèle | Compartiments nommés par l'org ; il faut **tous** les marquages d'une donnée pour la voir |
| Héritage | Automatique par le lignage ; retirer un marquage = déclassification explicite, justifiée, journalisée |
| Accès | Habilitation à durée limitée, rattachée à une finalité déclarée ; lectures journalisées sous elle |
| SAML | Attendre un client |
| Détection | Propose (une proposition à valider), ne change rien seule |
| Pseudonymisation | Colonne remplacée à l'écriture par une empreinte stable par org |
| Qui décide | Pas de rôle dédié (« c'est pas trop over engineering ? ») : l'admin d'org, tout journalisé |
| Ordre | Marquages d'abord, sur tables et objets ; I4 général ensuite |

## 3. Décisions

### D798 — Un marquage est un compartiment de l'org

`markings` : clé (anglais, `hr`, `health`), nom, description. Créés et retirés par l'admin d'org
(permission `governance:write`, niveau Admin ; lecture `governance:read`, niveau Viewer, pour voir la
liste des marquages, jamais les données). Un marquage porté par une donnée ne se supprime pas.

### D799 — Une habilitation = personne × marquage × finalité × échéance

`clearances` : utilisateur, marquage, finalité (texte libre obligatoire, 200 caractères), échéance
obligatoire (au plus 365 jours), auteur. Révocable. Une habilitation échue ne vaut plus rien, sans
tâche de nettoyage. Un admin peut s'habiliter lui-même : c'est journalisé comme le reste (choix de
Shan, pas de séparation des tâches en OP3). Les comptes de service ne sont jamais habilités (R22).

### D800 — Ce qui porte un marquage

- **Table** (dataset) : ses marquages valent pour toutes ses versions ;
- **Objet d'ontologie** : ses marquages propres ;
- Pas de marquage de colonne : une colonne sensible se sépare dans une autre table, ou se
  pseudonymise (D805).

### D801 — Lire = être habilité pour tous les marquages

- **Table** : métadonnées (nom, colonnes, versions, lignage) visibles comme aujourd'hui ; contenu
  (échantillon, profil, lecture d'une version, export) refusé sans habilitation complète
  (`403 marking-required`, `args.markings`).
- **Objet** : un lecteur non habilité reçoit l'objet caviardé par le masque existant de D443
  (titre `•••• <id>`, propriétés et attributs retirés, liste `redacted`), partout où ce masque
  s'applique déjà. Son existence reste visible : la cacher demande I4/I5 (limite assumée, §6).
- **Exécutions** : un pipeline, une build, une synchro lisent avec les droits de leur auteur ;
  l'auteur doit être habilité pour les marquages des entrées, revérifié à chaque run
  (`marking-required` sur le run).
- **Assistant, MCP, comptes de service** : mêmes routes, donc même refus.

### D802 — Le lignage propage, la déclassification est explicite

Une version de table écrite par un pipeline ou une build reçoit l'union des marquages de ses entrées ;
un objet écrit par la synchro d'une table reçoit les marquages de la table. Retirer un marquage d'une
table ou d'un objet : route dédiée, justification obligatoire, journalisée ; la propagation suivante
ne le remet que si une entrée le porte encore.

### D803 — Chaque lecture marquée laisse une trace

Flux O2 `governance_reads` : qui, quoi (table ou objet), quels marquages, sous quelle habilitation
(donc quelle finalité), par où (UI, MCP, clé, assistant, run). Pose, retrait, habilitation,
révocation et déclassification vont au même flux. Export CSV pour l'admin (`governance:read` ne
suffit pas : `governance:write`).

### D804 — La détection propose

Au chargement d'une version, le worker échantillonne chaque colonne et reconnaît e-mails,
téléphones, IBAN, numéros de carte (Luhn), NIR. Au-delà d'un seuil, une **proposition** (D371)
« marquer cette table » ou « pseudonymiser cette colonne » arrive dans « à valider ». Rien ne change
seul ; une proposition refusée ne revient pas pour la même colonne.

### D805 — La pseudonymisation remplace la valeur à l'écriture

Fonction SQL `pseudonym(col)` des pipelines : HMAC-SHA256 de la valeur avec une clé par org (au
coffre, jamais montrée), encodée en hexadécimal court. Stable dans l'org : les jointures marchent.
La valeur réelle n'entre pas dans la table dérivée, qui peut alors perdre le marquage par
déclassification justifiée (D802). Pas de réidentification dans PneX.

### D806 — Droit d'accès d'une personne

Depuis l'objet d'une personne : export JSON de tout ce que l'org en détient (propriétés, liens,
historique, mentions dans les dossiers et les pages), pour répondre à une demande d'accès. Réservé à
l'admin, journalisé. L'effacement existe déjà (D446).

## 4. Sécurité (grille `security.md` §6)

- R1 : org du principal ; R2 : `governance:*` au registre, refus testés ;
- habilitation vérifiée côté serveur à chaque lecture de contenu, jamais d'après l'UI ;
- les runs revérifient l'habilitation de leur auteur (une habilitation échue arrête une planification) ;
- R4/R16 : la clé de pseudonymisation ne sort jamais du serveur et du worker ;
- les traces ne contiennent pas les données lues, seulement leurs identifiants.

## 5. Lots

| Lot | Contenu | Livre |
|---|---|---|
| **G1** | D798, D799, D801 tables, D802 tables, D803 journal ; API + registre | Une table marquée refuse son échantillon à un non-habilité, sa table dérivée hérite |
| **G2** | D800–D802 objets : marquages d'objet, synchro qui propage, masque D443 étendu | Un objet issu d'une table marquée est caviardé pour un non-habilité |
| **G3** | Export du journal (D803) | CSV des lectures marquées d'une période |
| **G4** | D804 détection → propositions | Un CSV avec des e-mails produit une proposition |
| **G5** | D805 `pseudonym()` | Jointure sur deux tables pseudonymisées |
| **G6** | D806 export d'une personne | JSON complet depuis la fiche |
| **G7** | Écrans : marquages, habilitations (page Accès), marquages d'une table et d'un objet ; skills de design + `break-ui` | Parcours e2e complet |

## 6. Limites assumées

- L'existence d'un objet marqué reste visible (titre caviardé) tant qu'I4/I5 ne filtrent pas les
  listes ;
- pas de marquage de colonne (D800) ;
- pas de séparation des tâches : l'admin d'org peut s'habiliter, mais chaque geste est tracé.

## 7. Avancement

**G1 (2026-11-08)** : migration 000127 (`markings`, `clearances`, `dataset_markings`, trigger
`dataset_markings_inherit` sur `dataset_transaction_inputs`, en `SECURITY DEFINER` pour que le rôle
restreint du worker n'ait aucun droit sur les marquages), `pnex_core::governance`,
`services/governance.rs`, `controllers/governance.rs` (8 routes, `governance:read|write|delete`),
contrôle de `datasets_sample` et `datasets_profile`, champ `markings` du `Dataset`, journal O2
`governance_reads` par l'écrivain commun de l'audit (rendu générique), fiche `governance` de
l'assistant. Test `tests/governance.rs` : refus pour tous dont le propriétaire, habilitation,
isolation entre orgs (404 sans nommer les marquages), héritage par le trigger, déclassification
justifiée, marquage utilisé non supprimable, révocation immédiate.

Écart à relire : **pas de contrôle de l'auteur au lancement d'un run** en G1. Le lignage suffit à
fermer la fuite (une table calculée d'une table marquée est marquée, donc illisible pour un
non-habilité) ; la vérification de l'auteur vient avec G2, quand la synchro écrit des objets.

**G2 (2026-11-08)** : migration 000128 (`object_markings`), routes `ontology_objects_markings`,
`…_markings_add`, `…_declassify`. Le masque D443 (`Redaction`) porte l'ensemble des objets marqués
que l'appelant ne peut pas lire (`with_markings`, chargé une fois par requête) : objet caviardé entier
partout où le masque passe (lecture, liens, voisinage, hiérarchie, journal, dossiers, assistant) ;
**absent des requêtes et des agrégats** (CTE `vis` du plan de requête), sinon un filtre ou une somme
révélerait ses valeurs ; mise à jour et archivage refusés (`403 marking-required`). La synchro d'une
table marquée exige un auteur habilité (`marking-required` sur le run) et marque les objets écrits
**avant** de déclarer le run fini (une course sinon, trouvée par le test). `Redaction` n'est plus
`Copy` (sites adaptés). Tests : trois cas dans `tests/governance.rs` ; suites ontology,
personal_data, apps, ai, proposals, dataset_sync, connectors vertes.

Limites : l'existence d'un objet caviardé reste visible dans une lecture directe ou un lien ; un
parcours de graphe peut passer par un objet caché ; l'ensemble caché est chargé entier par requête
(`shortcut:` dans le code).
