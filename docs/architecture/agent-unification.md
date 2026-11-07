# Unification règles, navigation et agents (D780–D790)

> Décidé le 2026-11-07 (choix de Shan, même jour). **Rien n'est codé.** Ce document réunit
> trois mécanismes construits séparément : les recettes de site (`site-recipes.md`,
> D323–D328), les missions de l'agent navigateur (`extension-collector.md`, D254–D284,
> D333, D779) et les runs d'agents hébergés (`agent-environments.md`, D700–D775). Il
> remplace le lot A1 « missions → runs » d'`autonomous-agents.md` §17, jamais commencé,
> et met A2 (agents internes à LLM) en attente derrière lui.

## 1. Constat (2026-11-07)

| Concept | Recettes de site | Missions de l'extension | Runs d'agents hébergés |
|---|---|---|---|
| Où ça tourne | Navigateur du serveur seulement (CDP, `services/site_recipes/browser.rs`) | Extension seulement (`pnex-extension/src/wasm.rs` `run_mission`) | Bac à sable d'un runner |
| Ce qu'on exécute | YAML validé (`pnex_core::site_recipe`, 7 actions) | 8 types de mission codés en dur (`MissionSpec`) | Image + commande |
| État d'un passage | `site_recipe_runs.status` | `extension_missions.state`, sans historique | `runner_jobs` |
| Journal | `steps` JSON + captures S3 | aucun (code d'état) | `job_logs` |
| Bornes | 60 étapes, 11 min | plafonds par jour, cadence D258 | `llm_calls_max`, durée |
| Planification | nœud inject d'un flow | `interval_secs` + balayage horaire | flow ou API |
| Résultats | document médiathèque par extrait (D327) | tables propres (`site_pages`, `web_proofs`, captures de profil…) | `agent_artifacts` |

Deux fonctions savent déjà choisir entre le serveur et l'extension : l'exploration de site
(`CrawlExecutor::{Server, Agent}`) et la capture de preuve (extension, repli serveur). Ce
sont les précédents.

L'extension ne sait aujourd'hui ni cliquer, ni attendre, ni vérifier (`host.js` : ouvrir,
lire une valeur, lire la page, défiler, capturer, lire les liens).

## 2. Décisions

### D780 — Un seul format de règle : la recette

Le YAML des recettes de site (`pnex_core::site_recipe`, déjà pur et compilable en wasm)
est **le** format. Le navigateur du serveur et l'extension jouent la même recette. Les
missions codées en dur deviennent des **recettes fournies avec PneX** (catalogue en code,
versionnées comme les autres, copiables dans l'org pour être modifiées). Le LLM n'entre
pas dans une recette, sauf l'étape `llm_extract` (D328), qui reste optionnelle.

### D781 — Un run, plusieurs moteurs

Chaque exécution est un **run** du même type : même état, même journal (`job_logs`),
mêmes entrées et sorties (`agent_artifacts`), même arrêt, même budget, même page, même
nœud de flow. Seul le **moteur** change : `server_browser`, `extension`, `sandbox`, et
plus tard `internal_llm` (A2). La base est la paire existante `runner_jobs` + `agent_runs`.
Les passages de recettes et de missions y migrent, et `site_recipe_runs` disparaît
une fois migré (bêta : pas de compatibilité gardée).

### D782 — La recette déclare son exécuteur

Une recette déclare `session: required` (site qui exige l'utilisateur connecté, par
exemple LinkedIn) : elle part sur une extension de l'org. Sinon elle part sur le
navigateur du serveur, avec repli sur une extension si le site bloque le serveur. Le
lanceur (humain, flow, agent) peut forcer un exécuteur. Pas d'extension en ligne : le
run attend dans la file avec sa date limite, puis échoue avec un code.

### D783 — Tout est tenu côté serveur

L'extension n'a **aucune configuration** et ne fait **aucun aller-retour** de validation
pendant un run. Shan a testé : la validation et les allers-retours de configuration sont
trop longs.

- **Au lancement**, le serveur valide la recette et remplit ses paramètres. Il vérifie les
  hôtes autorisés (`allow_hosts`), l'opposition D341, les plafonds et la cadence, puis
  scelle le **plan** du run (YAML rempli, hôtes, bornes).
- L'extension reçoit ce plan au moment du bail et le joue jusqu'au bout.
- **Au retour**, le serveur revérifie chaque page visitée contre le plan (hôte, opposition)
  et écarte ce qui en sort, avec une ligne au journal. La cadence et les plafonds sont
  appliqués par la file du serveur, jamais par l'extension.

### D784 — Profil d'une personne par son nom, automatique

C'est une recette fournie (`linkedin_profile_by_name`) : recherche par prénom et nom
(société en option), ouverture du profil retenu, extraction par sélecteurs des écoles,
diplômes, postes et dates. **Sans LLM.** Elle peut être lancée par un humain, un flow ou
un agent.

**Levée explicite de D254 et D333 pour cette mission** (décision de Shan, 2026-11-07).
Avant, la capture était seulement humaine, faite à partir d'une URL, et la recherche
était interdite. Restent en place :

- l'opposition D341, vérifiée sur la personne et sur le profil retenu ;
- l'empreinte de titre D447, réutilisée par `msg.about`, D779 ;
- le plafond quotidien compilé par org et par réseau ;
- l'avertissement D254 dans l'UI.

Entrées (Shan, 2026-11-07) : **prénom, nom et entreprise**. Quand la recherche rend
plusieurs profils plausibles, la recette les capture **tous** (dans la limite du plafond)
et l'utilisateur **valide** celui qui correspond ; les autres sont écartés avec leur texte.

Les données extraites sont **proposées** (à valider), jamais écrites directement sur la
personne. Les conditions d'utilisation des plateformes restent sous la responsabilité de
l'org.

### D785 — Le LLM en dernier recours

Un agent interne à LLM (A2) n'est qu'un moteur de plus (`internal_llm`). Il n'est pas le
chemin par défaut. Une tâche qu'une recette sait faire passe par une recette, pour une
question de coût. A2 avance en parallèle des lots navigateur, dans ce modèle (moteur sur le run unifié).

### D786 — Un nœud de flow, une page

Le nœud `agent-run` lance n'importe quel run (agent hébergé, recette ou recette fournie).
`site-recipe` et la création de missions s'y replient. La page Agents montre tous les
runs, avec un filtre par moteur. La page des recettes reste le studio d'édition.

### D787 — Canaux de communication d'un agent (plus tard)

Un agent aura sa propre identité de communication : une boîte mail en entrée et en sortie,
qui fonctionne seule, peut-être un numéro de téléphone, et les connecteurs classiques
(Slack, Telegram…). Un message reçu lance un run, et le run peut répondre sur le même
canal. Ce sera fait **plus tard**, ce n'est pas une priorité (Shan, 2026-11-07). Ce qui
servira de base :

- les boîtes mail IMAP et SMTP (`mail.md`, D453–D462, envoi D518–D521) ;
- les canaux de notification (Slack, Matrix…) ;
- les comptes de service d'agent (D379).

Ce qui reste à décider avant de coder : l'envoi seul ou soumis à validation, selon l'agent ;
les quotas d'envoi ; la tenue de D123 (aucune action physique).

### D789 — Une règle tape dans un champ de recherche, jamais dans un formulaire (2026-11-07)

Choix de Shan : le format interdisait toute saisie (D254, D258). Une recherche par la barre
(D788) demande d'y taper. Nouvelle étape `type` : elle écrit seulement dans un **champ de
recherche** (`input type=search`, rôle `searchbox` ou `combobox`, champ d'une zone
`role=search`, ou champ dont le nom, l'étiquette ou l'indication dit « recherche »), en
général avec un paramètre de la règle. Un mot de passe, un champ d'un formulaire qui en
contient un, un champ d'identifiant ou d'adresse mail sont refusés au moment de jouer,
par les deux moteurs (`not-a-search-field`). Étape `press` : une touche d'une liste fermée
(`Enter`, `Tab`, `Escape`, flèches) ; `Enter` soumet le formulaire du champ quand la page
ne l'a pas fait. « Pas de connexion, pas de formulaire » reste vrai.

### D790 — Une liste de résultats et chacun de ses liens (2026-11-07)

Choix de Shan : posé dès U2, pour que le lecteur de l'extension soit écrit une fois.
`extract` gagne la forme `cards` : un sélecteur par carte, un sous-sélecteur par champ,
le texte ou un attribut d'une liste fermée (`href`, `src`, `title`, `alt`, `aria-label`,
`datetime`), liens rendus absolus, 50 cartes au plus. `for_each` ouvre chaque lien d'un
extrait `cards` (sur le site seulement, 10 liens au plus) et y joue ses étapes (10 au
plus, ni `open` ni boucle imbriquée) ; leurs extraits s'appellent `<as>.<n>`.

**Un seul code dans la page** (2026-11-07) : `pnex-core/js/rule_page.js` porte toutes les
fonctions qui tournent dans la page visitée (trouver, cliquer, taper, touche, cartes,
textes). Le navigateur du serveur l'embarque et l'évalue par CDP ; l'extension l'importe et
passe une fonction à `scripting.executeScript`, qui la sérialise seule : chaque fonction est
donc autonome. Les deux moteurs ne peuvent pas diverger sur ce que fait une étape dans la
page ; seul l'enchaînement des étapes est écrit deux fois (serveur, extension).

### D788 — Identifier une personne saisie par un commercial (2026-11-07)

Choix de Shan après le test de D779 sur dev (2026-11-07) : la capture d'une page
`/company/<nom>/people/` par un bouton ne sert qu'au test. L'adresse dépend d'un nom
LinkedIn qu'il faut deviner, et rien ne tourne seul. **L'entrée, c'est la personne** :
le commercial la saisit après un rendez-vous, une carte de visite, un salon, un mail,
rattachée à une entreprise. PneX fait la suite : recherche, ouverture, score, validation.
D784 devient ce pipeline. D779 reste comme outil de test manuel.

**Entrée.** Une `person` liée à une `organization` (`employs` ou `contact_of`), avec
prénom et nom. Une entreprise saisie en texte libre est d'abord résolue en organisation,
ou proposée.

**Garde avant toute recherche** (étape du pipeline, sans navigateur) :

- opposition D341 sur la personne et chez l'entreprise ;
- profil déjà validé sur la personne ;
- proposition en attente pour cette personne dans « À valider » ;
- recherche faite il y a moins de N jours (réglage du pack) ;
- périmètre : personnes des organisations au statut `target` ou `prospect`, meilleur
  `target_score` d'abord ;
- plafond compilé par org, par réseau et par jour (recherches et profils ouverts),
  cadence D258.

**Recherche par la barre de recherche, jamais par une adresse devinée.** Les requêtes
vont de la plus précise à la plus large. On s'arrête dès qu'un candidat dépasse le seuil
d'écriture.

1. « prénom nom » avec le filtre Personnes et l'entreprise actuelle, si sa page est
   connue ou résolue par la même barre ;
2. « prénom nom entreprise » sans filtre d'entreprise : la société est mal renseignée sur
   le réseau, ou rattachée à une autre page ;
3. « prénom nom » avec les autres noms de l'organisation : sigle, marque, `press_names`,
   nom d'usage, groupe, filiales et maison mère (hiérarchie GLEIF/SIRENE déjà en base),
   ancien nom ;
4. « prénom nom » seul, puis départagé par le lieu (villes des sites de l'organisation).

Chaque requête retient les N premiers candidats (réglage, 5 par défaut), dans une
limite de K profils ouverts par personne.

**Ouverture de chaque candidat retenu (profil complet).** Extraction par sélecteurs,
**sans LLM** (D785) : intitulé, lieu, postes (entreprise, titre, dates), écoles. Le texte
est gardé 30 jours, comme les profils capturés.

**Score de correspondance**, déterministe et expliqué (détail écrit comme
`score_detail`) :

- nom normalisé (accents, ordre prénom/nom, initiales, particules, nom composé) ;
- poste **actuel** chez l'organisation ou l'un de ses autres noms : fort ;
- poste **passé** chez l'organisation : moyen. C'est le cas du changement d'emploi, voir
  plus bas ;
- même groupe (filiale, maison mère) : moyen ;
- lieu cohérent avec un site de l'organisation : faible ;
- intitulé cohérent avec le persona saisi ou les personas visés : faible ;
- dates cohérentes avec la date de la rencontre (un poste commencé après la carte de
  visite ne l'explique pas).

**Écriture, sur le modèle de Présence web (deux seuils, réglages du pack)** :

- au-dessus du seuil haut : profil rattaché à la personne (adresse, postes, écoles), lien
  `employs` mis à jour, source tracée ;
- entre les deux seuils : proposition dans « À valider », avec tous les candidats et leur
  détail ; l'utilisateur choisit, les autres sont écartés avec leur texte ;
- sous le seuil bas : écarté, recherche suivante.

**Changement d'emploi.** Un candidat sûr dont le poste chez l'organisation est terminé
n'est jamais réécrit seul. On propose « a quitté X le …, est chez Y » : le lien `employs`
est fermé à sa date de fin, le lien vers Y est ouvert, et Y est créée comme `organization`
proposée si elle est inconnue.

**Rien trouvé** après toutes les requêtes : état `not_found` sur la personne, nouvel
essai après N jours. L'utilisateur peut coller l'adresse du profil, qui passe par la même
ouverture, le même score et la même écriture.

**Exécution.** Une recette fournie (`linkedin_person_match`) sur le moteur `extension`,
avec `session: required` (D782). Elle est lancée par un flow horaire du pack, *Prospecting
· identify people*, qui fait la garde, la file et l'écriture. La recette ne fait que
chercher, ouvrir et extraire. Il lui faut, dans `host.js`, la saisie dans un champ et la
validation (`type`, `press`), en plus de click, wait et assert (U2).

Restent en place : l'opposition D341 et l'empreinte D447, le plafond quotidien,
l'avertissement D254 dans l'UI, et la responsabilité de l'org pour les conditions
d'utilisation du réseau (suspension du compte connecté).

## 3. Lots

| Lot | Contenu | Dépend de |
|---|---|---|
| **U1** | Run unifié : moteur `server_browser` ; un passage de recette devient un run (état, `job_logs`, extraits en `agent_artifacts`) ; nœud `site-recipe` et page des recettes branchés dessus ; migration des passages existants | — |
| **U2** | Lecteur de recettes dans l'extension : primitives click, wait, assert, type, press (D788) dans `host.js` ; moteur `extension` ; plan scellé au bail (D783) ; contrôle des pages visitées au retour | U1 |
| **U3** | Routage D782 (`session: required`, repli, choix forcé) ; file et plafonds côté serveur | U2 |
| **U4** | Recettes fournies : `capture_profile`, `capture_people` (D779), `collect_page`… réécrites en recettes ; `linkedin_person_match` (D784 devenu D788) et flow *Prospecting · identify people* (garde, file, score, écriture) ; retrait des types de mission codés en dur | U3 |
| **U5** | Nœud `agent-run` unique et page Agents tous moteurs (D786) | U1 |
| **U6** | A2 : moteur `internal_llm`, outils de l'assistant, écritures en propositions, niveau dans la fiche agent (choix du 2026-11-07). Avancé en parallèle de U1–U4 (Shan, 2026-11-07) : il pose le moteur sur `runner_jobs` + `agent_runs`, comme D781. **Livré 2026-11-07** (voir §5) | — |
| **U7** (plus tard) | Canaux d'un agent : mail en entrée et en sortie, téléphone, Slack, Telegram (D787) | U5 |

**U2 en cours (2026-11-07).** Fait : format (`type`, `press`, `cards`, `for_each`, D789–D790)
et code de page partagé `rule_page.js` (1fb15e83) ; moteur `extension` côté serveur : un
passage lancé avec `engine: extension` est un job `extension` en file (30 min), loué par
le lien de l'agent qui peut le jouer (son navigateur au lanceur, sinon un commun, D550),
un à la fois, bail de 2 min renouvelé par chaque message ; le plan scellé (`RulePlan` :
règle relue, valeurs vérifiées, site) part dans `ServerMsg::Runs` ; l'agent renvoie
`run_step`, `run_extract`, `run_done` ; une étape hors du site et des hôtes permis termine
le passage en échec (`off-site`), un extrait hors plan ou d'un profil opposé (D341) est
écarté ; pas de capture d'écran ni de fichier stocké pour un agent ; bouton « Jouer dans
mon navigateur » du studio (93f3ac98). Lecteur dans l'extension (`pnex-extension/src/rules.rs`) :
il joue le plan reçu dans l'onglet de la plateforme du site (une règle sur LinkedIn partage
l'onglet et la cadence des missions LinkedIn, `rule_platform`), une étape à la fois au
rythme d'un lecteur, et appelle les fonctions de `rule_page.js` par `host.js`
(`rule_call`, import absolu `/rule_page.js` copié par `build-dist.sh`). Un site non autorisé
par l'utilisateur termine le passage en `permission-missing` et la page d'options le
demande. `follow_pdf` reste au navigateur du serveur (échec `browser-unavailable` sur un
agent, sauf `optional`). Vérifié : tests unitaires, protocole de bout en bout côté serveur
(`tests/extension.rs`), code de page dans un vrai Chrome (`page_side_…`) ; le cas e2e
`browser-agent.spec.ts` (extension réelle dans Chromium) tournera sur l'image suivante.
**U2 livré.**

**U3 livré (2026-11-07).** `session: required` dans le format (seule valeur) ; le moteur
d'un passage = le choix du lanceur, sinon l'agent pour une règle qui exige une session,
sinon le navigateur du serveur (`Recipe::engine`). Un passage pour un agent est refusé tout
de suite quand l'org n'a aucun agent appairé (`site-recipe-no-agent`) ou a déjà mis
100 passages en file dans la journée (`site-recipe-daily-cap`, plafond compilé). Repli : un
passage que le site bloque sur le navigateur du serveur (429, ou 403 sur l'étape) repart en
file pour un agent **sous le même id** (le studio le suit sans rien changer), sauf si le
lanceur a forcé `server_browser`. Le nœud de flow joue une règle à session sur un agent
**commun** (un flow n'a pas de personne dont il emprunterait le navigateur) et attend sa fin
18 min au plus (son appel en accorde 20). L'assistant ne joue jamais une règle à session
(`site-recipe-needs-session`) et ne passe jamais la main à un agent (moteur forcé).

**Notes pour U4 (`capture_people`, D779, poussé en 8b220d62).** Ce qu'une recette doit
reprendre sans rien perdre :

- Adresse : `browser_agent::people_page_of` (seule `linkedin.com/company/<nom>/people/`,
  canonique, refus `ext-target-not-people-page`) ; cible de bail `people_target`.
- Lecture : 5 écrans défilés au rythme D258 (`PEOPLE_SCROLLS`), puis le texte ; un renvoi
  hors de la page = `ext-login-required` / `ext-challenge` (`pnex-extension` `capture_page`).
- Plafond compilé `PEOPLE_CAPTURES_PER_DAY` = 10 par org et par jour, `ext-people-daily-cap`.
- Côté serveur : `services/profiles.rs` `request_people`, `people_captures`,
  `people_captured` (document `published_by` l'organisation, `people_to_read` posé) ;
  purge à 30 jours par `metadata.people_org_id`.
- Lecture par le flow *Prospecting · captured employee lists* (`prospecting_b2b` 3.5.0) ;
  `object_upsert` reçoit `msg.about` : un nom opposé chez cet objet n'est pas recréé
  (D447). La recette `linkedin_profile_by_name` (D784) peut réutiliser ce contrôle.
- Test de bout en bout côté serveur : `an_employee_list_lands_on_its_company_and_skips_objections`.

## 4. Risques

- Les sites qui bloquent l'automatisation bloquent aussi l'extension si son rythme n'est
  pas humain. La cadence D258 doit passer dans la file du serveur sans être affaiblie.
- Le contrôle au retour (D783) arrive après coup : une page hors plan a déjà été ouverte
  dans le navigateur de l'utilisateur. On l'accepte, parce que le plan est scellé et que
  l'extension ne joue que lui.
- Il faut un plan de migration des missions en cours (D779 est poussé par une autre
  session) avant le retrait des types codés en dur, prévu à U4.

## 5. U6 livré (2026-11-07)

- **Fiche** : `AgentSpec.level` (`hosted` par défaut, `internal`), `environment_id` facultatif
  (exigé en hébergé), `tools` (noms du registre de l'assistant, jeton `unknown_tool` sinon ;
  outil d'écriture refusé à un agent lecteur, `write_needs_member`), `max_steps` (20 par
  défaut, 1 à 50). Un agent interne refuse tout champ du bac (`hosted_only`) : ni image, ni
  commande, ni variable, ni dépôt, ni service, ni cache, ni sortie déclarée.
- **Run** : job `internal_llm` joué par une tâche du backend (`services/internal_agents.rs`,
  `FOR UPDATE SKIP LOCKED`, bail = durée du run + 2 min), jamais pris par un runner.
  `AgentRun.engine` = le genre du job. Arrêt vu avant chaque appel au modèle ; délai = celui
  du run ; `take_call` + quota quotidien `llm-agent` avant chaque appel, `add_tokens` après.
- **Boucle** : celle de l'assistant (`ai::agent::run_loop` sous une politique) ; le chat
  garde son comportement. `loop_guard` : même outil + mêmes arguments 3 fois → échec
  `agent-loop-detected`.
- **Écritures** : chaque appel d'un outil d'écriture devient une proposition `tool_call`
  (origine `agent:<id>`, `run_id` dans les valeurs). L'accepter rejoue les arguments exacts
  par `tools::execute` avec les droits de la personne qui décide (comme `run_confirmed`) ;
  un refus de l'outil la laisse ouverte (409, code de l'outil ou `proposal-tool-failed`).
  Pas de correction des valeurs.
- **Entrées** : le texte des fichiers d'entrée (64 Kio chacun au plus, balisé comme donnée)
  est la tâche ; les compétences activées épinglées sont les consignes. La réponse finale
  est la sortie `report`.
- **Écarts assumés** : pas de planification, pas de priorités (D400), pas d'étapes
  reprenables (D397 : un backend tué rejoue le run du début après expiration du bail) ; le
  journal part dans OpenObserve à la fin du run, pas au fil de l'eau.
