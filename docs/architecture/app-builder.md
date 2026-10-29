# Applications construites dans PneX : écrans métier sur l'ontologie (D506–D513)

> **Statut : PRD validé par Shan le 2026-10-29, rien d'implémenté.**
> Questions Q-B1 à Q-B6 tranchées : les défauts (§10). Le code attend le
> feu vert de Shan pour le lot AB0.
>
> Demande de Shan (2026-10-29) : faire évoluer la partie supervision
> (dashboards, surfaces, contrôles), pensée pour l'IoT, vers un builder
> d'applications métier générique, sans code, dans l'esprit d'Appsmith.
> L'état des lieux conclut : oui, mais **sur l'ontologie** (modèle Foundry
> Workshop), pas sur des bases externes. Arbitrage de Shan sur le mot :
> « dashboard » fait outil de BI ; la notion est l'**application**.
>
> C'est le prolongement de D432–D441 (`external-apps.md`) : l'application
> D433 (périmètre, ensembles d'objets, actions permises) gagne une
> interface construite dans PneX, à côté de l'interface externe par le
> SDK. Une notion, un mot (lexique D218).
>
> Liens : `external-apps.md` (D432–D441), `viz-bases.md` (D24, D31, D40,
> D41), `surfaces-controls.md` (D123–D133), `ontology.md` (D183 actions,
> D185 requête, D187 dashboards de type, D188 droits), `iam.md`,
> `editor-shell.md`, `design-system.md` §7 (lexique), `security.md`
> (R1–R23), `ai-assistant.md` §9, `api-contract.md`, `mcp.md`.

## 1. Constat (relevé du 2026-10-29)

| Brique d'un builder | Existant | Manque |
|---|---|---|
| Notion d'application | `apps` (D433) : clé, nom, périmètre, ensembles d'objets nommés (D434) ; compte de service **optionnel** ; un pack peut livrer une application | Des écrans, une navigation, un lanceur |
| Éditeur d'écran | `EditorShell`, canvas libre (D40), formats PC et mobile, palette, inspecteur, versions (dashboards) | — |
| Requête d'objets | `POST /ontology/query` (D185) ; `pnex_core::apps::bind` substitue les paramètres `$nom` ; `services::apps::run_set` exécute un ensemble dans le périmètre | Exécution d'un ensemble par un **humain** (aujourd'hui `/apps/me/…`, compte de service seulement) |
| Fiche d'un objet | Dashboard de type (D187) : `DashboardLayout.object_type` + sélecteur | Ouverture depuis une ligne de tableau |
| Écriture | `ontology_objects_create` / `update` (validation du type, `expected_version`, erreurs codées) ; `POST /ontology/objects/{id}/actions/{key}` (D183) | Widgets qui les appellent ; liste d'actions permises (D440) |
| Widgets | 17 types orientés métriques et contrôles (`dashboard_editor/library.rs`) | Tableau, fiche, compteur, saisie, formulaire, bouton d'action |

## 2. Principe (D506)

### D506 — Une application a une interface construite, externe, ou les deux

L'application reste l'objet D433 de l'org. Elle gagne :

- des **écrans** ordonnés (`screens`), chacun avec une clé, un nom, une
  icône et son document de mise en page ;
- un **écran d'accueil** (`home`) ;
- ses requêtes restent ses **ensembles d'objets** (D434), ses écritures
  ses **actions permises** (D440).

Une application peut n'avoir que des écrans (aucun compte de service),
que le SDK (compte de service, pas d'écran, cas de Vigie), ou les deux.

**Dashboard et application ne se confondent pas** :

| | Dashboard | Application |
|---|---|---|
| Usage | Regarder : supervision, métriques, séries, contrôles d'org | Travailler : chercher, consulter, saisir des objets, déclencher des actions |
| Données | Séries (`SourceRef`), mémoire, contrôles | Ensembles d'objets de l'application, dans son périmètre |
| Où | Menu Dashboards, chrome PneX | Lanceur Applications, plein écran avec sa propre navigation |

Les deux partagent le moteur de rendu, l'éditeur (`EditorShell`) et les
widgets de métriques : un écran d'application peut afficher une jauge.
Les widgets de données (D510, D511) n'existent que dans les écrans
d'application, parce qu'ils lisent les ensembles d'objets.

Généricité (D441) : aucun métier dans le moteur ; exemples, modèles et
tests sur l'ontologie inventée (`article`, `auteur`, lien `ecrit_par`).

## 3. Écrans (D507)

### D507 — Un écran est un document de mise en page versionné, rattaché à son application

- Stockage : l'écran réutilise `dashboards` / `dashboard_versions` avec
  une colonne `app_id` (nullable, `ON DELETE CASCADE`) : versions,
  conflits 409, éditeur et validation sont ceux des dashboards. Un écran
  n'apparaît pas dans la liste des dashboards.
- L'application porte l'ordre et le nom des écrans (`screens`), sa
  version change quand on ajoute, retire ou réordonne un écran.
- Publication : Q-B2.

## 4. Lire des objets (D508)

### D508 — Un widget de données lit un ensemble d'objets de son application, avec les droits du lecteur

- `WidgetOptions.object_set: Option<String>` (nom d'un ensemble de
  l'application) + `params` (variable D509 par paramètre `$nom`).
- Nouvelle route `GET /api/v1/apps/{id}/object-sets/{name}?limit&offset&<paramètres>`
  pour un humain : même `run_set`, droits du lecteur (org depuis le
  principal, R1 ; types verrouillés masqués, D188), **intersectés** avec
  le périmètre de l'application. Une application ne donne aucun droit ;
  elle en retire.
- Paramètres : valeurs seulement (`bind`), jamais une clé, un opérateur ni
  une traversée ; variable vide → le widget affiche « filtre à
  renseigner » et n'envoie rien.
- Pagination du moteur (500 au plus) ; jamais de liste complète au front.
- Les ensembles d'objets deviennent éditables dans le builder (choix du
  type, filtres, tri), plus seulement en JSON dans la page Accès.
- Validation au save : ensemble, variables et propriétés citées existent.

## 5. Variables (D509)

### D509 — Les widgets d'une application communiquent par des variables déclarées

- `variables: Vec<Variable>` au niveau de l'**application** (partagées
  entre écrans) : `name` (`[a-z][a-z0-9_]*`), `kind` (`text`, `number`,
  `date`, `object`, `enum`), `type_key` pour `object`, `default`.
- Écrites par : un champ de saisie, la sélection d'une ligne de tableau
  (variable `object`), le paramètre d'URL `?v.<nom>=…` (deep-link).
- Lues par : les paramètres des ensembles d'objets, les fiches, les
  formulaires, les boutons d'action.
- État local à la session de l'utilisateur, jamais en base. Une variable
  est une valeur, jamais une expression (D512).

## 6. Widgets de données (D510, D511)

### D510 — Quatre widgets de lecture et de saisie

| Widget | Rôle |
|---|---|
| `object_table` | Colonnes = titre + propriétés choisies, tri, pages ; un clic sur une ligne écrit la variable `object` et peut ouvrir un autre écran |
| `object_fields` | Fiche : propriétés choisies d'un objet (variable `object`), lien vers la page objet |
| `object_count` | Nombre d'objets d'un ensemble (`count` du résultat paginé), seuils comme `stat` |
| `input` | Champ typé (texte, nombre, date, enum du type) qui écrit une variable |

Valeurs affichées en texte, jamais en HTML (R11–R13).

### D511 — Deux widgets d'écriture, par les routes existantes et la liste d'actions permises

| Widget | Appelle | Garde |
|---|---|---|
| `object_form` | `ontology_objects_create` ou `update` sur `expected_version` | Type dans le périmètre ; validation du type ; erreurs de champ sous le champ ; 409 → « modifié entre-temps, recharger » |
| `action_button` | `POST /ontology/objects/{id}/actions/{key}` (D183) sur la variable `object` | Action dans la liste permise de l'application (D440) ; dialogue D183 existant ; 202 = « envoyé à N flows » |

- Pas de nouvelle route d'écriture pour l'humain : mêmes services, mêmes
  droits (`can_write`, `write_role` du type), mêmes 409. Le périmètre et
  la liste d'actions de l'application sont vérifiés côté serveur (en-tête
  ou paramètre `app` sur ces routes, détail au lot AB3), pas seulement
  par l'interface.
- **Précision de D123** : « une surface lit, un flow agit » vise l'action
  physique. Écrire une donnée de l'ontologie depuis une application est
  permis, comme depuis la page objet ; agir sur un device reste réservé
  aux flows déployés (un `action_button` publie vers un flow).

## 7. Ce qui n'entre pas (D512)

### D512 — Ni source externe, ni code, ni HTML dans une application

- **Pas de connecteur SQL, REST ou GraphQL** : les données externes
  entrent par l'ingestion et les flows (D317), avec leur provenance
  (D184). C'est ce qui distingue une application PneX d'Appsmith.
- **Pas de JS, de Starlark ni d'expression `{{ }}`** dans un widget : la
  logique vit dans les flows et les fonctions, versionnés et débogables.
- **Pas de HTML ni de CSS libre** (R11–R13).
- Pas de synchronisation git, pas de partage hors de l'org (Q-B4).

## 8. Exécution, contrat, droits, assistant (D513)

### D513 — Le lanceur, la vue plein écran et les gardes habituelles

- **Lanceur** : entrée de menu « Applications » (lexique : le mot existe
  déjà, même notion) qui liste les applications ayant au moins un écran.
- **Vue** `/apps/{key}/{screen}` : plein écran, navigation entre écrans
  (barre latérale PC, onglets en bas en mobile), retour à PneX.
- **Routes** : `GET /apps/{id}/object-sets/{name}` et l'édition des
  écrans entrent au spec (`#[utoipa::path]`, `task openapi`), à
  `exposure.rs` (lecture exposée au MCP, écriture de structure exclue,
  motif accès) et à `iam.rs` (Q-B1 : qui construit).
- **Assistant** : fiche `assistant-kb` « applications » (écrans,
  variables, widgets, exemple sur l'ontologie inventée) ; l'assistant
  peut proposer un écran en brouillon, jamais publier ; un
  `action_button` reste une proposition (D183, D144).
- **Packs** : un pack qui livre une application (`apps:`, D433) peut
  livrer ses écrans : une application métier arrive prête à l'emploi.
- Lexique : « Écran », « Variable » et chaque libellé visible ajoutés au
  lexique et à `lexicon_guard` ; clés dans les deux `.ftl`.

## 9. Lots

| Lot | Contenu | Tests |
|---|---|---|
| **AB0** | `pnex-core` : `screens`, `home`, `variables` sur l'application ; `dashboards.app_id` ; `WidgetOptions.object_set` ; route humaine d'ensemble d'objets ; IAM, exposition, spec | intersection périmètre × droits ; 403 sans permission ; variable inconnue refusée |
| **AB1** | Builder : écrans d'une application dans `EditorShell`, éditeur d'ensembles d'objets ; lanceur et vue plein écran, deep-link | e2e : créer une application, deux écrans, naviguer |
| **AB2** | `object_table`, `input`, `object_count`, `object_fields` ; ligne → variable et écran cible | e2e : saisie → tableau filtré → fiche ; type verrouillé masqué |
| **AB3** | `object_form` + contrôle serveur du périmètre sur l'écriture | e2e : création, conflit 409, viewer → widget désactivé, type hors périmètre → 403 |
| **AB4** | D440 liste d'actions permises + `action_button` | e2e : 202 ; action non permise refusée ; 409 `ontology-action-no-flow` affiché |
| **AB5** | Fiche assistant, lexique, application modèle (liste + fiche + formulaire) sur l'ontologie inventée, écrans livrés par pack, mobile | gardes kb/lexique ; `engine_genericity` |

Premier lot de valeur : AB0 → AB2 (application de consultation, sans
écriture).

## 10. Questions tranchées (Shan, 2026-10-29 : les défauts)

| # | Question | Décision |
|---|---|---|
| Q-B1 | Qui construit une application ? Aujourd'hui `apps:write` = Admin. | Nouvelle permission `apps:build` (Editor) pour les écrans et ensembles ; périmètre et actions permises restent Admin (`apps:write`) |
| Q-B2 | Publication : les utilisateurs voient-ils la dernière version enregistrée ou une version publiée ? | Version publiée (pointeur, école flows/tours) : on construit sans casser ce que les autres utilisent |
| Q-B3 | Qui voit une application dans le lanceur ? | Tout membre de l'org en V1 ; restriction par rôle plus tard |
| Q-B4 | Partager une application hors de l'org (lien public) ? | Non en V1 |
| Q-B5 | Résultat d'une action : D183 est sans retour. Un retour du flow pour l'écran ? | Non en V1 : « envoyé », puis rechargement |
| Q-B6 | Mobile ? | PC d'abord (AB1–AB4), mobile en AB5 |
