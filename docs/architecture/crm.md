# Suivi commercial : données personnelles et pack `crm` (D442–D452)

| | |
|---|---|
| Statut | **Proposition — 2026-10-26**, code en cours (lots DP puis CRM1…CRM3) |
| Déclencheur | Outillage commercial (P2.28) : après la cartographie et les signaux, le suivi des opportunités, des activités et des relances, sans ressaisie dans un second outil. Avant de vendre : D341 fini |
| Repose sur | `account-mapping.md` (D329–D343, pack `prospecting_b2b`), `proposals.md` (D367, D371–D377), `ontology.md` (D176–D191, D368 réglages de pack, D370 libellés d'énumération), `iam.md` (D298 classification, D296 portées), `partner-watch.md` (nœuds ontologie D312) |
| Numérotation | **D442…D452**, lots **DP**, **CRM1…CRM3** ; **D514…D517**, lot **CRM4** ; **D522** (2026-10-29) |
| Revient sur | le non-objectif « Pas de CRM » d'`account-mapping.md` §2 (D448) |

## 1. Le besoin en une phrase

Suivre une affaire de la liste cible au contrat dans le même graphe que le compte : l'opportunité, qui y participe, ce qui a été fait et ce qu'il reste à faire, avec des relances préparées par la plateforme et envoyées par un humain. Avant cela, tenir les engagements de D341 sur les données personnelles.

## 2. Non-objectifs

- **Pas d'émission de facture légale ni de contrat** : la facture (numérotation sans trou, inaltérabilité, Factur-X, plateforme agréée de la réforme 2026) est émise par l'outil de facturation de l'org ; PneX suit les devis et les factures (D516, revu le 2026-10-29).
- **Aucun envoi automatique** de mail ni de message sur un réseau social : la plateforme prépare, l'humain envoie et coche (règle des tickets, CGU des réseaux).
- **Pas de surveillance automatique des personnes** (D254, D333 inchangés).
- **Pas de playbook dans le pack** : étapes, critères de qualification, plan de relance et personas sont des données de l'org (règle « le pack donne la forme, l'org donne le playbook »).
- **Pas de synchro CRM/ERP** dans ce PRD (D369, lot L6 de la note produit).
- **Rien de propre au CRM dans le moteur** (D441) : le noyau gagne seulement des briques génériques (classification, masquage, effacement, jetons de date).

## 3. Données personnelles : fin de D341 (lot DP)

### D442 — Classification D298 codée

`PropertyDef` gagne `classification` : `public`, `internal` (défaut, omis), `personal`, `sensitive` (liste fermée, iam.md D298). Les attributs de type de lien sont des `PropertyDef` : même champ.

`ObjectTypeDef` gagne `title_classification` (même liste, défaut `internal`) : le titre d'une personne est son nom.

Packs livrés classés : `investigation` (personne : alias, rôle, notes, titre ; `employs.position`), `prospecting_b2b` (personne : persona), `companies` (dirigeant : `manages` sans attribut personnel ajouté). Une valeur hors liste est refusée à l'installation (`field-invalid`).

### D443 — Masquage par rôle

Deux permissions de lecture, inscrites au registre IAM :

| Permission | Plus bas rôle | Ce qu'elle laisse voir |
|---|---|---|
| `ontology.personal:read` | Lecteur | propriétés, attributs et titres `personal` |
| `ontology.sensitive:read` | Administrateur | propriétés, attributs et titres `sensitive` |

Un rôle personnalisé sans la permission reçoit les objets **caviardés** : propriétés et attributs de lien retirés, titre remplacé par `•••• <8 caractères de l'id>` (neutre, sans traduction), et la liste `redacted` des clés retirées sur l'objet. Points d'application : lecture d'un objet, résolution, liens, voisinage, chemin, impact, nœuds de la hiérarchie, requête D185, historique, flux des changements (D435), ensembles d'objets (D434), propositions (leurs objets), outils `query_ontology` et `get_object` de l'assistant ; le MCP passe par les mêmes routes. Les écritures ne changent pas.

Étendu le 2026-10-26 : les autres parents d'un nœud de hiérarchie (`also_under`, masqués à la construction) et toutes les lectures des dossiers d'enquête (espace de travail, tableau, signaux, traces, propositions, relations, export, instantanés, journal des lectures, et les outils de l'assistant qui les lisent) passent par le même masque, un parcours du JSON qui masque tout objet (`id` + `type_key`) et tout lien qu'il nomme. Une page générée depuis un objet est remplie avec le masque de son auteur (SEC-53, 2026-10-27). Limite restante : elle garde le texte que son auteur pouvait lire ; la masquer pour ses lecteurs suivants demande la visibilité par objet (iam.md I5).

### D444 — Base légale et durée de rétention : réglages du pack

Deux réglages D368 de `prospecting_b2b` :

- `legal_basis` (texte, défaut `legitimate_interest_b2b_prospecting`) : affichée sur la fiche d'une personne ;
- `retention_months` (nombre, défaut 36, de 6 à 120).

### D445 — File de rétention : des propositions d'effacement

Chaque nuit, pour chaque org où `prospecting_b2b` est installé : une personne sans modification, sans lien ouvert ou fermé et sans capture depuis `retention_months` devient une **proposition d'effacement** (nouveau genre `object_erase` de D371) dans la boîte « à valider », avec sa dernière activité en preuve.

- **Jamais acceptée par une règle d'org** (D377 refusé pour ce genre) : l'effacement est une action humaine.
- Accepter = effacer (D446). Refuser = garder ; la date du refus compte comme une activité, la personne ne revient pas avant une nouvelle période.
- Une seule proposition ouverte par personne.

### D446 — Effacement en coquille

`POST /ontology/objects/{id}/erase` (permission `ontology:delete`), générique :

- propriétés `personal` et `sensitive` supprimées, titre `personal`/`sensitive` remplacé par `∅ <8 caractères de l'id>`, attributs personnels de ses liens supprimés ;
- colonne `objects.erased_at` posée ; l'objet reste (coquille), ses liens et la provenance des autres objets tiennent ; **la coquille est en lecture seule** : toute modification et tout nouveau lien sont refusés (`ontology-erased`, 409), un import en masse laisse le lien de côté (constaté à l'essai du 2026-10-26 : un `PUT` remettait le nom effacé) ;
- l'historique d'un objet effacé est relu caviardé (propriétés classées et titre) ; le journal O2 garde l'ancien contenu jusqu'à sa rétention (D72), sans le servir ;
- journalisé `object.erase`, sans le contenu effacé.

Pour une personne (service des profils, exempté de la garde D441) : ses identifiants de compte et ses textes de profil capturés sont archivés, les mails dont elle est le correspondant (l'origine d'un transfert cru, sinon l'expéditeur) sont supprimés, jamais le transfert d'un membre qui la citerait, et elle entre dans la liste d'opposition (D447).

Le MCP ne l'expose pas (motif destructif).

### D447 — Opposition consultée partout

- `oppose` pose aussi la propriété `opposed_on` (date, pack `prospecting_b2b`) sur la personne : les flows de relance la filtrent.
- La liste d'opposition gagne une empreinte **nom + compte** : `title:<nom normalisé>@<id de l'purpose lié>`, posée pour chaque organisation liée à la personne au moment de l'opposition ou de l'effacement.
- **Le moteur refuse tout nouveau lien** entre un objet dont le titre est `personal` et un objet de l'empreinte quand le nom normalisé correspond (`prospecting-opposed`, 409) : un rapprochement qui recrée « la même personne chez le même compte » échoue. Dans un import en masse, le lien est laissé de côté et le reste du lot passe (un refus ne fait pas échouer l'import). Générique : aucune clé de type ou de lien n'est nommée.
- Nom normalisé : minuscules, accents latins repliés (« Élodie » = « elodie » = « ELODIE », `œ` → `oe`, apostrophe typographique), lettres et chiffres seulement, un espace entre les mots.
- **Une seule table de repli** (`pnex_core::fold`) sert aux empreintes, au texte de la recherche globale et de la requête D185 (titre et filtre `contains`) : la colonne y est repliée en SQL par `translate(lower(…))`, majuscules accentuées comprises. Aucune extension Postgres (`unaccent`), aucune dépendance. Les deux copies qui existaient (`taxonomy`, `browser_agent`) y délèguent. Coût : la recherche par sous-chaîne n'utilisait déjà aucun index.

## 4. Le pack `crm` (lots CRM1…CRM3)

### D448 — Le CRM est un pack

Nouveau pack `crm` (`requires: [prospecting_b2b]`). Il revient sur le non-objectif « Pas de CRM » d'`account-mapping.md` §2 en le gardant hors du noyau. Le pipeline reste simple : la valeur est dans le compte vu comme un réseau, les signaux et le rendez-vous.

### D449 — Opportunité et qualification

Type `opportunity` :

| Propriété | Forme |
|---|---|
| `stage` | énumération : `discovery`, `qualification`, `proposal`, `negotiation`, `won`, `lost` (clés stables, l'org les renomme par D370) |
| `amount`, `probability` (0–100), `expected_close` (date) | |
| `source` | `trade_show`, `buying_group`, `inbound`, `outbound`, `referral`, `network`, `other` ; `source_detail` (nom du salon…) |
| `loss_reason` | texte |
| `last_activity` | date-heure, écrite par flow |
| `missing_criteria` | texte, écrit par flow |
| `follow_up_started_on` | date, écrite par flow |

Liens : `opportunity_with` (→ `organization`, un seul ouvert), `involved_in` (`person` → opportunité, attribut `role` = persona), `owned_by` (opportunité → `person` interne), `comes_from` (opportunité → `sales_signal`, `meeting`), `meeting_for` (`meeting` → opportunité).

**Qualification = données de l'org** : type `qualification_criterion` (description, `stage` à partir de laquelle il est exigé, actif). Lien `qualified_on` (opportunité → critère, attributs `status` `unknown`/`partial`/`valid`, `note`). SPICED, MEDDPICC ou la grille maison sont des listes de critères ; le pack n'en livre aucune. Un flow écrit `missing_criteria` : les critères exigés jusqu'à l'étape courante qui ne sont pas `valid`. Rien n'est bloqué : le passage d'étape reste une décision humaine.

### D450 — Activités et cadence

Type `activity` : `ref` (unique), `kind` (`call`, `mail`, `meeting`, `network_message`, `note`, `task`), `state` (`to_do`, `done`, `cancelled`), `due_on` (date), `done_at` (date-heure), `direction` (`inbound`, `outbound`), `content`, `draft` (texte préparé par le LLM de l'org), `reported` (bool, écrit par flow). Lien `activity_on` (→ opportunité, organisation, personne).

Le **plan de relance** est une donnée de l'org : type `follow_up_step` (`day` depuis le lancement, `kind`, `instruction`, actif). Flows livrés :

- **Cadence** (toutes les heures) : une opportunité en `qualification` sans `follow_up_started_on` reçoit une activité `to_do` par étape active, d'échéance « lancement + jour », liée à l'opportunité ; puis `follow_up_started_on`. Une opportunité dont tous les contacts impliqués ont fait opposition est sautée.
- **Brouillons** (toutes les heures) : une activité `mail` ou `network_message` à faire, due d'ici demain, sans brouillon, reçoit une accroche rédigée par le LLM de l'org à partir de la consigne et du compte. **L'humain la relit et l'envoie lui-même**, puis coche `done`.
- **Dernière activité** (toutes les 15 minutes) : une activité faite et non reportée met à jour `last_activity` de son opportunité.
- **Critères manquants** (toutes les heures) : D449.

### D451 — « Ma journée » et alertes : un digest du matin

Flow « CRM · ma journée » (7 h) : un texte pour le nœud de notification de l'org, avec les relances dues aujourd'hui ou en retard et les opportunités ouvertes sans activité depuis 14 jours (modifiable dans le flow). Même forme que le digest des signaux (D335), qui reste le digest des signaux : les deux ne sont pas fusionnés.

Brique générique pour cela : les jetons `$today-<n>` et `$today+<n>` (jours, 0 à 3650) dans les filtres des nœuds `ontology_query`, résolus par le runtime comme `$today`.

### D452 — Modèle de page `opportunity_sheet`

Rempli depuis l'opportunité (D340) : compte, étape, montant, contacts impliqués, critères et leur statut, activités, rendez-vous, puis les prochaines étapes en texte libre. Export PDF par `page_pdf`.

## 4 bis. Pistes, devis et factures (lot CRM4, 2026-10-29)

Demande de Shan (2026-10-29) : « la partie CRM / facturation / gestion des leads ». Arbitrage : devis et suivi des factures dans le pack, pas d'émission légale.

### D514 — La piste (`lead`) avant l'opportunité

Type `lead` : `ref` (unique, facultative : clé d'import d'un fichier de salon), `status` (`new`, `contacted`, `qualified`, `disqualified`, `converted`), `source` et `source_detail` (mêmes valeurs que l'opportunité), `company`, `contact_name`, `email`, `phone`, `message`, `received_on`, `disqualify_reason`. Titre et coordonnées classés `personal` (D442) : une piste nomme une personne qui n'est pas encore dans le graphe.

Liens : `lead_of` (→ `organization`, un seul ouvert), `lead_contact` (→ `person`, un seul), `owned_by` et `activity_on` acceptent la piste, `comes_from` accepte la piste comme origine d'une opportunité.

Entrées : saisie à la main, import (Ontologie › Imports, fichier de salon). Pas de création depuis les mails reçus (le pourriel deviendrait des pistes) ni de formulaire public en V1.

### D515 — Convertir : une action, un flow livré

Action `convert` sur la piste (`organization` exigé, `person` et `name` facultatifs), servie par le flow modèle « CRM · convert a lead » (D183, `action_trigger`) : opportunité en `discovery`, source reprise, titre = nom donné, sinon la société, jamais le nom du contact ; liens `opportunity_with`, `comes_from`, `lead_of`, `lead_contact`, `involved_in` ; piste `converted`. L'opportunité porte une nouvelle propriété `ref` = `lead:<id de la piste>` (unique) : une seconde conversion retrouve la même. Le compte est choisi par l'humain (créé avant au besoin) : pas de rapprochement automatique au moment de la conversion. Aucune donnée personnelle n'est recopiée sur l'opportunité.

### D516 — Devis et factures suivis, émis ailleurs

Revient en partie sur le non-objectif §2. Type `quote` : `ref` (numéro, unique, exigé), `state` (`draft`, `sent`, `accepted`, `refused`, `expired`), `amount` (HT), `issued_on`, `valid_until`, `content` ; lien `quote_for` (→ opportunité, une seule). Modèle de page `quote_sheet` (lignes à compléter, export PDF par `page_pdf`) : le devis peut naître dans PneX. Type `invoice` : `ref` (numéro de l'outil de facturation, unique, exigé), `state` (`issued`, `partially_paid`, `paid`, `overdue`, `cancelled`), `amount` (TTC), `paid_amount`, `issued_on`, `due_on`, `paid_on`, `url` (la facture dans l'outil) ; liens `invoice_for` (→ opportunité, compte) et `invoices_quote` (→ devis). La facture entre à la main ou par import ; la synchro avec l'outil attend les connecteurs D369.

Rien n'est automatique entre devis, facture et étape : un devis accepté ne fait pas gagner l'opportunité (D449, le passage d'étape reste humain).

### D517 — Échéances et digest

Flow modèle « CRM · quotes and invoices due » (6 h 30) : devis `sent` dont `valid_until` est passée → `expired` ; facture `issued` ou `partially_paid` dont `due_on` est passée → `overdue` et une activité `task` à faire aujourd'hui (`ref` `invoice:<id>:overdue`, une seule par facture) sur ce qu'elle facture. « CRM · my day » liste aussi les pistes `new`.

### D522 — Brouillons de relance dans la boîte d'envoi

Flow modèle « CRM · drafts to the outbox » (toutes les heures, 20 au plus) : une activité `mail` à faire qui a un brouillon (D451) devient un mail de la boîte d'envoi (nœud `mail_send`, `mail.md` D518–D521) de la boîte choisie au déploiement. Destinataires : les identifiants `email` des personnes impliquées dans l'opportunité de l'activité, sauf celles qui se sont opposées (`opposed_on`, D447) ; objet : le nom de l'opportunité ; clé `crm:<ref>` (jamais deux mails pour une activité). L'activité garde l'id du mail (`outbox_mail`), ou `none` quand personne n'a d'adresse. Une personne relit, envoie ou écarte dans Boîtes mail › Boîte d'envoi, puis passe l'activité à faite : rien n'est envoyé sans elle, sauf boîte partagée réglée sur l'envoi direct (D520). Les messages réseau restent des brouillons sur l'activité.


| Lot | Décisions | Sortie |
|---|---|---|
| DP | D442–D447 | D341 fini : classification, masquage, base légale, file de rétention, effacement, opposition au moteur |
| CRM1 | D448, D449, D452 | pack installable : opportunités, qualification, fiche |
| CRM2 | D450, D451 | activités, cadence, brouillons, digest du matin |
| CRM4 | D514–D517, D522 | pistes et conversion, devis et factures suivis, échéances, brouillons vers la boîte d'envoi |
| CRM3 | — | mail entrant rattaché à la fiche : devient le lot M6 de `mail.md` (boîtes mail génériques, D453–D462) ; tableaux de bord (ROI par salon, taux par étape de relance) à part |

## 6. Sécurité et conformité

- Org depuis le principal (R1) ; chaque route au registre IAM (R2) ; aucun identifiant dans les DTO (R4).
- L'effacement n'est jamais automatique (D445) et n'est pas exposé au MCP (D446).
- Le contenu effacé ne part pas dans le journal (D446).
- Les brouillons sont des propositions de texte, jamais envoyés par la plateforme (D450).

## 7. Questions ouvertes

1. **Q-CRM1 — Purge O2** : faut-il aussi supprimer du journal O2 les lignes d'un objet effacé avant la fin de la rétention, si O2 le permet ?
2. **Q-CRM2 — Titres dans le graphe** : en attendant I5, masquer aussi les titres `personal` dans le graphe, la hiérarchie et les chemins ?
3. ~~**Q-CRM3 — Mail**~~ : IMAP retenu (Shan, 2026-10-26), OAuth2 XOAUTH2 ensuite pour Microsoft 365 et Google ; conception générique dans `mail.md`.

Q-CRM1 et Q-CRM2 : sans avis de Shan (2026-10-26), les défauts prudents tiennent — pas de purge O2 (journal borné par sa rétention, jamais relu en clair) ; masquage actuel (voisinage, chemins, impact, nœuds de hiérarchie), le reste avec I5.

## 8. Récapitulatif des décisions

| # | Décision |
|---|---|
| D442 | Classification D298 codée (propriétés, attributs, titre) |
| D443 | Masquage par rôle : `ontology.personal:read`, `ontology.sensitive:read` |
| D444 | Base légale et rétention : réglages de `prospecting_b2b` |
| D445 | File de rétention = propositions `object_erase`, jamais automatiques |
| D446 | Effacement en coquille, historique relu caviardé |
| D447 | Opposition au moteur : empreinte nom + compte, `opposed_on` |
| D448 | Le CRM est un pack |
| D449 | Opportunité et qualification par critères de l'org |
| D450 | Activités, plan de relance, brouillons relus |
| D451 | Digest « ma journée », jetons `$today±n` |
| D452 | Modèle de page `opportunity_sheet` |
| D514 | Type `lead`, coordonnées `personal` |
| D515 | Action `convert` et son flow modèle, opportunité clé `lead:<id>` |
| D516 | Devis et factures suivis, émission légale hors PneX, modèle `quote_sheet` |
| D517 | Devis expirés, factures en retard avec relance, pistes au digest |
| D522 | Brouillons de relance mail dans la boîte d'envoi, sans les personnes opposées |

## 9. État d'implémentation

### Lot DP — 2026-10-26, non commité

| Brique | Où | Tests |
|---|---|---|
| D442 `classification` + `title_classification` | `pnex-core` `ontology/schema.rs`, `ontology/mod.rs` ; packs `investigation` 1.11.0, `prospecting_b2b` 1.3.0 | désérialisation fermée (valeur inconnue refusée) |
| D443 masquage | `services/ontology/redact.rs` ; contrôleurs `ontology`, `ontology_feed`, `apps`, `proposals` ; outils de l'assistant ; permissions au registre IAM + `GATES` | unitaires `redact` ; `tests/personal_data.rs` (rôle refusé : titre, propriétés, attributs, requête) |
| D444 réglages | `prospecting_b2b.yaml` `settings` | — |
| D445 file de rétention | `services/profiles.rs` `propose_erasures` (boucle horaire du purgeur, singleton) ; genre `object_erase` (`pnex-core` `proposal.rs`, contrainte SQL étendue) ; `erase::decide` | `personal_data.rs` (proposée une fois, acceptée = effacée, refusée = période relancée) ; vérifiée en réel le 2026-10-27 sur la stack Docker (personne vieillie de 40 mois, proposée au démarrage du serveur avec la durée de l'org, acceptée = coquille, même archivée) |
| D446 effacement | `services/ontology/erase.rs`, `POST /ontology/objects/{id}/erase` (`ontology:delete`, exclu du MCP : destructif) ; colonne `objects.erased_at` (migration 000057) ; historique relu caviardé ; `profiles::before_erase` | `personal_data.rs` (coquille, idempotent, attribut de lien retiré) |
| D447 opposition au moteur | `services/ontology/opposition.rs` ; contrôle dans `links::create_with` et `create_many_with` (lien laissé de côté) ; `opposed_on` posé par `oppose` | `personal_data.rs` (même nom au même compte refusé, ailleurs accepté) |
| Repli des accents (D447) | `pnex-core` `fold.rs` ; `opposition.rs`, `query.rs`, `controllers/global_search.rs` | unitaires `fold` ; `tests/crm.rs` `names_match_without_accents_or_case` |
| Jetons `$today±n` (D451) | `pnex-core` `ontology/flow.rs`, doc du nœud `ontology_query` | unitaire `today_offsets_resolve` |

Reste : l'export du registre des traitements (pas de registre dans PneX aujourd'hui), le masquage des titres dans les dossiers et les pages générées (I5), l'écran (bouton « Effacer », base légale affichée sur la fiche, masques).

### Lots CRM1–CRM2 — 2026-10-26, non commité

Pack `crm` 1.0.0 (`packs/crm.yaml`) : types, liens, hiérarchie `opportunities`, modèle `opportunity_sheet`, 11 fonctions Starlark, 5 flows modèles (plan de relance, dernière activité, critères manquants, brouillons, ma journée). Tests : `pnex-node-starlark/tests/pack_rules.rs` (échéances à travers les fins de mois et années bissextiles, opposition, critères, dernière activité, digest) et `tests/crm.rs` (installation, brouillons de flows, cardinalité, hiérarchie, fiche). Fiches de l'assistant `sales-follow-up` et `personal-data` ; lexique §7 + garde.

### Lot CRM4 — 2026-10-29, non commité

Pack `crm` 3.1.0 : types `lead`, `quote`, `invoice`, propriété `opportunity.ref`, liens `lead_of`, `lead_contact`, `quote_for`, `invoice_for`, `invoices_quote`, action `convert`, modèle `quote_sheet`, 6 fonctions Starlark, 2 flows modèles (9 au total). Tests : `pack_rules.rs` (`a_converted_lead_becomes_an_opportunity_of_its_account`, `an_overdue_invoice_is_marked_and_reminded`, digest avec pistes) et `tests/crm.rs` (piste, cardinalités, numéros uniques, page devis, action refusée sans flow déployé).

Limites : la file de rétention (D445) ne propose pas encore l'effacement des pistes (personnes seulement) ; une piste s'efface à la main (D446). Pas de piste créée depuis un mail ou un formulaire public. Brouillons de relance branchés sur la boîte d'envoi (D522) : objet = nom de l'opportunité, non modifiable avant envoi ; une adresse ajoutée après le marquage `none` n'est pas reprise. Restent pour le CRM : tableaux de bord (ROI par salon, conversion par étape) et écrans métier, qui attendent les applications AB0–AB2 (`app-builder.md`).
