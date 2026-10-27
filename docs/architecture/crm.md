# Suivi commercial : données personnelles et pack `crm` (D442–D452)

| | |
|---|---|
| Statut | **Proposition — 2026-10-26**, code en cours (lots DP puis CRM1…CRM3) |
| Déclencheur | Outillage commercial (P2.28) : après la cartographie et les signaux, le suivi des opportunités, des activités et des relances, sans ressaisie dans un second outil. Avant de vendre : D341 fini |
| Repose sur | `account-mapping.md` (D329–D343, pack `prospection_b2b`), `proposals.md` (D367, D371–D377), `ontology.md` (D176–D191, D368 réglages de pack, D370 libellés d'énumération), `iam.md` (D298 classification, D296 portées), `partner-watch.md` (nœuds ontologie D312) |
| Numérotation | **D442…D452**, lots **DP**, **CRM1…CRM3** |
| Revient sur | le non-objectif « Pas de CRM » d'`account-mapping.md` §2 (D448) |

## 1. Le besoin en une phrase

Suivre une affaire de la liste cible au contrat dans le même graphe que le compte : l'opportunité, qui y participe, ce qui a été fait et ce qu'il reste à faire, avec des relances préparées par la plateforme et envoyées par un humain. Avant cela, tenir les engagements de D341 sur les données personnelles.

## 2. Non-objectifs

- **Pas de devis, de facturation ni de contrat** : un PDF de page suffit en V1 ; la facturation reste ailleurs.
- **Aucun envoi automatique** de mail ni de message sur un réseau social : la plateforme prépare, l'humain envoie et coche (règle des tickets, CGU des réseaux).
- **Pas de surveillance automatique des personnes** (D254, D333 inchangés).
- **Pas de playbook dans le pack** : étapes, critères de qualification, plan de relance et personas sont des données de l'org (règle « le pack donne la forme, l'org donne le playbook »).
- **Pas de synchro CRM/ERP** dans ce PRD (D369, lot L6 de la note produit).
- **Rien de propre au CRM dans le moteur** (D441) : le noyau gagne seulement des briques génériques (classification, masquage, effacement, jetons de date).

## 3. Données personnelles : fin de D341 (lot DP)

### D442 — Classification D298 codée

`PropertyDef` gagne `classification` : `publique`, `interne` (défaut, omis), `personnelle`, `sensible` (liste fermée, iam.md D298). Les attributs de type de lien sont des `PropertyDef` : même champ.

`ObjectTypeDef` gagne `title_classification` (même liste, défaut `interne`) : le titre d'une personne est son nom.

Packs livrés classés : `investigation` (personne : alias, rôle, notes, titre ; `employs.position`), `prospection_b2b` (personne : persona), `entreprises` (dirigeant : `dirige` sans attribut personnel ajouté). Une valeur hors liste est refusée à l'installation (`field-invalid`).

### D443 — Masquage par rôle

Deux permissions de lecture, inscrites au registre IAM :

| Permission | Plus bas rôle | Ce qu'elle laisse voir |
|---|---|---|
| `ontology.personal:read` | Lecteur | propriétés, attributs et titres `personnelle` |
| `ontology.sensitive:read` | Administrateur | propriétés, attributs et titres `sensible` |

Un rôle personnalisé sans la permission reçoit les objets **caviardés** : propriétés et attributs de lien retirés, titre remplacé par `•••• <8 caractères de l'id>` (neutre, sans traduction), et la liste `redacted` des clés retirées sur l'objet. Points d'application : lecture d'un objet, résolution, liens, voisinage, chemin, impact, nœuds de la hiérarchie, requête D185, historique, flux des changements (D435), ensembles d'objets (D434), propositions (leurs objets), outils `query_ontology` et `get_object` de l'assistant ; le MCP passe par les mêmes routes. Les écritures ne changent pas.

Étendu le 2026-10-26 : les autres parents d'un nœud de hiérarchie (`also_under`, masqués à la construction) et toutes les lectures des dossiers d'enquête (espace de travail, tableau, signaux, traces, propositions, relations, export, instantanés, journal des lectures, et les outils de l'assistant qui les lisent) passent par le même masque, un parcours du JSON qui masque tout objet (`id` + `type_key`) et tout lien qu'il nomme. Une page générée depuis un objet est remplie avec le masque de son auteur (SEC-53, 2026-10-27). Limite restante : elle garde le texte que son auteur pouvait lire ; la masquer pour ses lecteurs suivants demande la visibilité par objet (iam.md I5).

### D444 — Base légale et durée de rétention : réglages du pack

Deux réglages D368 de `prospection_b2b` :

- `base_legale` (texte, défaut `interet_legitime_prospection_b2b`) : affichée sur la fiche d'une personne ;
- `retention_mois` (nombre, défaut 36, de 6 à 120).

### D445 — File de rétention : des propositions d'effacement

Chaque nuit, pour chaque org où `prospection_b2b` est installé : une personne sans modification, sans lien ouvert ou fermé et sans capture depuis `retention_mois` devient une **proposition d'effacement** (nouveau genre `object_erase` de D371) dans la boîte « à valider », avec sa dernière activité en preuve.

- **Jamais acceptée par une règle d'org** (D377 refusé pour ce genre) : l'effacement est une action humaine.
- Accepter = effacer (D446). Refuser = garder ; la date du refus compte comme une activité, la personne ne revient pas avant une nouvelle période.
- Une seule proposition ouverte par personne.

### D446 — Effacement en coquille

`POST /ontology/objects/{id}/erase` (permission `ontology:delete`), générique :

- propriétés `personnelle` et `sensible` supprimées, titre `personnelle`/`sensible` remplacé par `∅ <8 caractères de l'id>`, attributs personnels de ses liens supprimés ;
- colonne `objects.erased_at` posée ; l'objet reste (coquille), ses liens et la provenance des autres objets tiennent ; **la coquille est en lecture seule** : toute modification et tout nouveau lien sont refusés (`ontology-erased`, 409), un import en masse laisse le lien de côté (constaté à l'essai du 2026-10-26 : un `PUT` remettait le nom effacé) ;
- l'historique d'un objet effacé est relu caviardé (propriétés classées et titre) ; le journal O2 garde l'ancien contenu jusqu'à sa rétention (D72), sans le servir ;
- journalisé `object.erase`, sans le contenu effacé.

Pour une personne (service des profils, exempté de la garde D441) : ses identifiants de compte et ses textes de profil capturés sont archivés, les mails dont elle est le correspondant (l'origine d'un transfert cru, sinon l'expéditeur) sont supprimés, jamais le transfert d'un membre qui la citerait, et elle entre dans la liste d'opposition (D447).

Le MCP ne l'expose pas (motif destructif).

### D447 — Opposition consultée partout

- `oppose` pose aussi la propriété `opposition_le` (date, pack `prospection_b2b`) sur la personne : les flows de relance la filtrent.
- La liste d'opposition gagne une empreinte **nom + compte** : `title:<nom normalisé>@<id de l'objet lié>`, posée pour chaque organisation liée à la personne au moment de l'opposition ou de l'effacement.
- **Le moteur refuse tout nouveau lien** entre un objet dont le titre est `personnelle` et un objet de l'empreinte quand le nom normalisé correspond (`prospection-opposed`, 409) : un rapprochement qui recrée « la même personne chez le même compte » échoue. Dans un import en masse, le lien est laissé de côté et le reste du lot passe (un refus ne fait pas échouer l'import). Générique : aucune clé de type ou de lien n'est nommée.
- Nom normalisé : minuscules, accents latins repliés (« Élodie » = « elodie » = « ELODIE », `œ` → `oe`, apostrophe typographique), lettres et chiffres seulement, un espace entre les mots.
- **Une seule table de repli** (`pnex_core::fold`) sert aux empreintes, au texte de la recherche globale et de la requête D185 (titre et filtre `contains`) : la colonne y est repliée en SQL par `translate(lower(…))`, majuscules accentuées comprises. Aucune extension Postgres (`unaccent`), aucune dépendance. Les deux copies qui existaient (`taxonomy`, `browser_agent`) y délèguent. Coût : la recherche par sous-chaîne n'utilisait déjà aucun index.

## 4. Le pack `crm` (lots CRM1…CRM3)

### D448 — Le CRM est un pack

Nouveau pack `crm` (`requires: [prospection_b2b]`). Il revient sur le non-objectif « Pas de CRM » d'`account-mapping.md` §2 en le gardant hors du noyau. Le pipeline reste simple : la valeur est dans le compte vu comme un réseau, les signaux et le rendez-vous.

### D449 — Opportunité et qualification

Type `opportunite` :

| Propriété | Forme |
|---|---|
| `etape` | énumération : `decouverte`, `qualification`, `proposition`, `negociation`, `gagnee`, `perdue` (clés stables, l'org les renomme par D370) |
| `montant`, `probabilite` (0–100), `cloture_prevue` (date) | |
| `source` | `salon`, `groupement`, `entrant`, `sortant`, `recommandation`, `reseau`, `autre` ; `source_detail` (nom du salon…) |
| `motif_perte` | texte |
| `derniere_activite` | date-heure, écrite par flow |
| `criteres_manquants` | texte, écrit par flow |
| `cadence_lancee_le` | date, écrite par flow |

Liens : `opportunite_de` (→ `organization`, un seul ouvert), `implique` (`person` → opportunité, attribut `role` = persona), `porte_par` (opportunité → `person` interne), `issue_de` (opportunité → `signal_commercial`, `rendez_vous`), `rdv_pour` (`rendez_vous` → opportunité).

**Qualification = données de l'org** : type `critere_qualification` (description, `etape` à partir de laquelle il est exigé, actif). Lien `qualifie` (opportunité → critère, attributs `statut` `inconnu`/`partiel`/`valide`, `note`). SPICED, MEDDPICC ou la grille maison sont des listes de critères ; le pack n'en livre aucune. Un flow écrit `criteres_manquants` : les critères exigés jusqu'à l'étape courante qui ne sont pas `valide`. Rien n'est bloqué : le passage d'étape reste une décision humaine.

### D450 — Activités et cadence

Type `activite` : `cle` (unique), `kind` (`appel`, `mail`, `rendez_vous`, `message_reseau`, `note`, `tache`), `etat` (`a_faire`, `faite`, `annulee`), `echeance` (date), `faite_le` (date-heure), `sens` (`entrant`, `sortant`), `contenu`, `brouillon` (texte préparé par le LLM de l'org), `reportee` (bool, écrit par flow). Lien `activite_sur` (→ opportunité, organisation, personne).

Le **plan de relance** est une donnée de l'org : type `etape_cadence` (`jour` depuis le lancement, `kind`, `consigne`, actif). Flows livrés :

- **Cadence** (toutes les heures) : une opportunité en `qualification` sans `cadence_lancee_le` reçoit une activité `a_faire` par étape active, d'échéance « lancement + jour », liée à l'opportunité ; puis `cadence_lancee_le`. Une opportunité dont tous les contacts impliqués ont fait opposition est sautée.
- **Brouillons** (toutes les heures) : une activité `mail` ou `message_reseau` à faire, due d'ici demain, sans brouillon, reçoit une accroche rédigée par le LLM de l'org à partir de la consigne et du compte. **L'humain la relit et l'envoie lui-même**, puis coche `faite`.
- **Dernière activité** (toutes les 15 minutes) : une activité faite et non reportée met à jour `derniere_activite` de son opportunité.
- **Critères manquants** (toutes les heures) : D449.

### D451 — « Ma journée » et alertes : un digest du matin

Flow « CRM · ma journée » (7 h) : un texte pour le nœud de notification de l'org, avec les relances dues aujourd'hui ou en retard et les opportunités ouvertes sans activité depuis 14 jours (modifiable dans le flow). Même forme que le digest des signaux (D335), qui reste le digest des signaux : les deux ne sont pas fusionnés.

Brique générique pour cela : les jetons `$today-<n>` et `$today+<n>` (jours, 0 à 3650) dans les filtres des nœuds `ontology_query`, résolus par le runtime comme `$today`.

### D452 — Modèle de page `fiche_opportunite`

Rempli depuis l'opportunité (D340) : compte, étape, montant, contacts impliqués, critères et leur statut, activités, rendez-vous, puis les prochaines étapes en texte libre. Export PDF par `page_pdf`.

## 5. Lots

| Lot | Décisions | Sortie |
|---|---|---|
| DP | D442–D447 | D341 fini : classification, masquage, base légale, file de rétention, effacement, opposition au moteur |
| CRM1 | D448, D449, D452 | pack installable : opportunités, qualification, fiche |
| CRM2 | D450, D451 | activités, cadence, brouillons, digest du matin |
| CRM3 | — | mail entrant rattaché à la fiche : devient le lot M6 de `mail.md` (boîtes mail génériques, D453–D462) ; tableaux de bord (ROI par salon, taux par étape de relance) à part |

## 6. Sécurité et conformité

- Org depuis le principal (R1) ; chaque route au registre IAM (R2) ; aucun identifiant dans les DTO (R4).
- L'effacement n'est jamais automatique (D445) et n'est pas exposé au MCP (D446).
- Le contenu effacé ne part pas dans le journal (D446).
- Les brouillons sont des propositions de texte, jamais envoyés par la plateforme (D450).

## 7. Questions ouvertes

1. **Q-CRM1 — Purge O2** : faut-il aussi supprimer du journal O2 les lignes d'un objet effacé avant la fin de la rétention, si O2 le permet ?
2. **Q-CRM2 — Titres dans le graphe** : en attendant I5, masquer aussi les titres `personnelle` dans le graphe, la hiérarchie et les chemins ?
3. ~~**Q-CRM3 — Mail**~~ : IMAP retenu (Shan, 2026-10-26), OAuth2 XOAUTH2 ensuite pour Microsoft 365 et Google ; conception générique dans `mail.md`.

Q-CRM1 et Q-CRM2 : sans avis de Shan (2026-10-26), les défauts prudents tiennent — pas de purge O2 (journal borné par sa rétention, jamais relu en clair) ; masquage actuel (voisinage, chemins, impact, nœuds de hiérarchie), le reste avec I5.

## 8. Récapitulatif des décisions

| # | Décision |
|---|---|
| D442 | Classification D298 codée (propriétés, attributs, titre) |
| D443 | Masquage par rôle : `ontology.personal:read`, `ontology.sensitive:read` |
| D444 | Base légale et rétention : réglages de `prospection_b2b` |
| D445 | File de rétention = propositions `object_erase`, jamais automatiques |
| D446 | Effacement en coquille, historique relu caviardé |
| D447 | Opposition au moteur : empreinte nom + compte, `opposition_le` |
| D448 | Le CRM est un pack |
| D449 | Opportunité et qualification par critères de l'org |
| D450 | Activités, plan de relance, brouillons relus |
| D451 | Digest « ma journée », jetons `$today±n` |
| D452 | Modèle de page `fiche_opportunite` |

## 9. État d'implémentation

### Lot DP — 2026-10-26, non commité

| Brique | Où | Tests |
|---|---|---|
| D442 `classification` + `title_classification` | `pnex-core` `ontology/schema.rs`, `ontology/mod.rs` ; packs `investigation` 1.11.0, `prospection_b2b` 1.3.0 | désérialisation fermée (valeur inconnue refusée) |
| D443 masquage | `services/ontology/redact.rs` ; contrôleurs `ontology`, `ontology_feed`, `apps`, `proposals` ; outils de l'assistant ; permissions au registre IAM + `GATES` | unitaires `redact` ; `tests/personal_data.rs` (rôle refusé : titre, propriétés, attributs, requête) |
| D444 réglages | `prospection_b2b.yaml` `settings` | — |
| D445 file de rétention | `services/profiles.rs` `propose_erasures` (boucle horaire du purgeur, singleton) ; genre `object_erase` (`pnex-core` `proposal.rs`, contrainte SQL étendue) ; `erase::decide` | `personal_data.rs` (proposée une fois, acceptée = effacée, refusée = période relancée) |
| D446 effacement | `services/ontology/erase.rs`, `POST /ontology/objects/{id}/erase` (`ontology:delete`, exclu du MCP : destructif) ; colonne `objects.erased_at` (migration 000057) ; historique relu caviardé ; `profiles::before_erase` | `personal_data.rs` (coquille, idempotent, attribut de lien retiré) |
| D447 opposition au moteur | `services/ontology/opposition.rs` ; contrôle dans `links::create_with` et `create_many_with` (lien laissé de côté) ; `opposition_le` posé par `oppose` | `personal_data.rs` (même nom au même compte refusé, ailleurs accepté) |
| Repli des accents (D447) | `pnex-core` `fold.rs` ; `opposition.rs`, `query.rs`, `controllers/global_search.rs` | unitaires `fold` ; `tests/crm.rs` `names_match_without_accents_or_case` |
| Jetons `$today±n` (D451) | `pnex-core` `ontology/flow.rs`, doc du nœud `ontology_query` | unitaire `today_offsets_resolve` |

Reste : l'export du registre des traitements (pas de registre dans PneX aujourd'hui), le masquage des titres dans les dossiers et les pages générées (I5), l'écran (bouton « Effacer », base légale affichée sur la fiche, masques).

### Lots CRM1–CRM2 — 2026-10-26, non commité

Pack `crm` 1.0.0 (`packs/crm.yaml`) : types, liens, hiérarchie `opportunites`, modèle `fiche_opportunite`, 11 fonctions Starlark, 5 flows modèles (plan de relance, dernière activité, critères manquants, brouillons, ma journée). Tests : `pnex-node-starlark/tests/pack_rules.rs` (échéances à travers les fins de mois et années bissextiles, opposition, critères, dernière activité, digest) et `tests/crm.rs` (installation, brouillons de flows, cardinalité, hiérarchie, fiche). Fiches de l'assistant `sales-follow-up` et `personal-data` ; lexique §7 + garde.
