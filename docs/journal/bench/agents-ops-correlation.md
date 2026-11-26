# Benchmark — Agents autonomes et corrélation exploitation/sécurité

**Date :** 2026-11-24 · **Pour :** `docs/architecture/it-operations.md` (D1001–D1016) et `docs/architecture/agents.md` §3.19 (D1031–D1037).

## 0. Méthode et limites

- Lecture des docs publiques et des dépôts, le 2026-11-24. Rien n'a été installé ni lancé.
- Une partie des faits vient de la mémoire du modèle (mi-2026). Ils sont marqués **[mém]**. Un fait non vérifié est marqué **[nv]**.
- Les produits d'agents évoluent vite. Revérifier un fait avant toute citation publique.

## 1. Outils d'agents

| Outil | Rôle | Ce qu'il fait | Licence |
|---|---|---|---|
| LangChain, LangGraph | Bibliothèque pour coder un agent | Graphe d'états, checkpoint et reprise, interruption humaine, mémoire courte et longue, streaming | Open source [nv pour le nom de licence] |
| LangSmith | Plateforme | Traces, évaluations, jeux de test, passerelle LLM, passerelle d'outils, registre d'agents, Managed Deep Agents avec cron | Propriétaire |
| Langfuse | Observabilité LLM | Traces en arbre, sessions, gestion des prompts avec étiquettes, jeux de test, expériences, LLM-juge, scores, files d'annotation, coûts, passerelle IA (bêta) | MIT, modules `ee/` sous licence commerciale |
| Paperclip | Orchestration d'agents « en entreprise » | Organigramme, objectifs, projets, tickets, budgets par portée, validations du « board », routines cron/webhook/API, adaptateurs de harnais (Claude Code, Codex, Gemini CLI, OpenCode…), compétences versionnées | MIT |

## 2. Agents lancés par un horaire ou un événement

| Produit | Déclencheurs | Harnais | Garde avant écriture | Licence |
|---|---|---|---|---|
| OpenHands | Ticket, mention, cron, webhook | Le sien, modèle au choix | Demande de fusion | MIT, `enterprise/` à part |
| Copilot coding agent | Ticket assigné, chat, API | Le sien, autres via Agent HQ [mém] | Brouillon de demande de fusion, CI de l'agent non lancée sans accord, pare-feu de sortie par défaut | Propriétaire |
| Claude Code (Action, routines) | Événement GitHub, cron, API | Claude Code | Modes de permission, hooks | Propriétaire, Action MIT [mém] |
| Codex cloud | Web, Slack, GitHub, Linear | Codex | Secrets retirés avant la phase agent, internet coupé | Propriétaire |
| Devin | Slack, tickets, API, cron | Devin | Revue de la demande de fusion, plafonds ACU par org | Propriétaire |
| Jules | Web, API, tâches planifiées | Jules | Plan validé, puis demande de fusion | Propriétaire |
| Cursor Automations | Cron, forge, Slack, Linear, PagerDuty, webhook | Cursor | Demande de fusion | Propriétaire |
| Paperclip | Routines cron, webhook, API | Adaptateurs au choix | Validations du board, budgets | MIT |
| kagent | CRD Kubernetes, A2A | Le sien | [nv] | Apache-2.0 |
| n8n, Windmill, Temporal | Cron, webhook, files | Étape d'agent | Étapes d'approbation | n8n fair-code, Windmill AGPLv3 + EE, Temporal MIT |

## 3. Corrélation exploitation et sécurité

| Produit | Topologie | Corrélation | Couche réseau | Licence |
|---|---|---|---|---|
| Coroot | Découverte eBPF + API Kubernetes | Règles sur la topologie, puis LLM | Latence entre nœuds | Apache-2.0 |
| Keep | Fournisseurs d'alertes | Fenêtre de temps, règles, topologie, LLM | Non | MIT + `ee/` [mém], racheté par Elastic en 2025 |
| HolmesGPT | Aucune, requêtes à la demande | LLM avec outils | Non | Apache-2.0, CNCF Sandbox |
| K8sGPT | API Kubernetes | Règles, puis LLM | Non | Apache-2.0 |
| Collecteur OTel servicegraph, Jaeger | Déduite des traces | Aucune | Non | Apache-2.0 |
| Backstage | Déclarée (`catalog-info.yaml`) | Aucune | Non | Apache-2.0 |
| NetBox, Nautobot | Déclarée, découverte en option | Aucune | Oui : sites, baies, câbles, interfaces | Apache-2.0 |
| OpenCTI | Graphe STIX, connecteurs | Règles d'inférence | Non | Apache-2.0 + édition entreprise |
| Wazuh | Inventaire des agents | Règles de fréquence et de fenêtre | Hôte | GPLv2 |
| OpenSearch Security Analytics | Aucune | Règles Sigma, corrélation entre types de logs | Par les logs | Apache-2.0 |
| Cartography | Découverte par API cloud et SaaS, Neo4j | Requêtes de graphe | Réseau cloud | Apache-2.0, CNCF Sandbox |
| BloodHound CE | Collecteurs AD, Entra | Chemins d'attaque | Non | Apache-2.0 |
| DefectDojo, Dependency-Track | Déclarée (scans, SBOM) | Dédoublonnage, correspondance de composants | Non | BSD-3, Apache-2.0 |

Références propriétaires, pour leurs modèles seulement :
- Dynatrace Smartscape + Davis : la topologie d'abord, puis la causalité par parcours des dépendances.
- ServiceNow CMDB + Service Mapping : relations de configuration avec un sens d'impact, règles de réconciliation.
- Splunk ITSI : la santé remonte vers les services parents.
- Datadog : les flux réseau sont rattachés aux services, donc une panne réseau montre son impact applicatif.
- Wiz, Orca : un risque est un chemin du graphe (« combinaison toxique »), pas une faille seule.
- BigPanda, PagerDuty : un regroupement d'alertes explicable, appris des fusions humaines.

**Constat** : aucun projet open source ne réunit les trois briques suivantes :
- une topologie physique, logicielle et sécurité dans un même graphe
- des connecteurs vers les stores de logs et le SIEM existants
- des agents planifiés avec un harnais au choix.

Les seules offres complètes sont propriétaires, et le harnais y est imposé.

## 4. Retenu pour PneX

| Idée | Vue chez | Où dans PneX |
|---|---|---|
| La topologie d'abord. Une fenêtre de temps départage seulement. | Dynatrace, ITSI, Coroot | `it-operations.md` D1006 |
| Déclaré et découvert, avec la provenance par lien | ServiceNow, Backstage, NetBox | D1004 (même règle que D856) |
| Le risque est un chemin du graphe | Wiz, BloodHound, Cartography | D1009 |
| Des lecteurs avec curseur : PIT et `search_after`, list et watch (relist sur 410), fenêtres de temps pour Loki | Keep, Cartography | D1002 |
| Regrouper les alertes, puis un seul run d'agent par incident | Keep, BigPanda | D1010 |
| Le LLM explique un chemin que le graphe a trouvé | Constat sur les outils LLM seuls | D1007 |
| Traces des runs en arbre, en conventions OTel GenAI | Langfuse, LangSmith | `agents.md` D1031 |
| Export OTLP vers l'outil de l'org | Langfuse, kagent | D1032 |
| Jeux de cas, évaluateurs (règle, LLM-juge, humain), comparaison de versions | Langfuse, LangSmith | D1033 |
| Reprise d'un run par étape, pause humaine | LangGraph, Temporal | D1034 (lève D397 pour le moteur interne) |
| AGENTS.md et SKILL.md comme contrat de consignes | Copilot, Codex, OpenHands, Claude | Déjà couvert : compétences épinglées (D770–D774). Import git : `agents.md` §7. |
| Secrets hors de la phase agent, egress en liste blanche | Codex, Copilot | Déjà couvert : D705, `setup` sans secret, R8 |
| Budget dur par org et par agent | Devin, Paperclip | Déjà couvert : `llm_calls_max`, compteur quotidien, budget de chaîne |

## 5. Écarté

| Idée | Raison |
|---|---|
| Un organigramme d'agents (rôles, hiérarchie, agent PDG, objectifs) | Un rôle est une compétence avec un titre. La délégation est un flow (`agent_run`, `agent_run_finished`). Le budget existe par chaîne et par run, et les validations par la porte (D833). Écarté par Shan le 2026-11-24. |
| Un réveil périodique des agents (heartbeat) pour sonder | Cela brûle des tokens. Paperclip est passé aux routines. PneX déclenche sur un événement (flow). |
| Une notion « routine » à part | Un flow avec `inject` périodique, `ontology_trigger` ou un webhook fait déjà la même chose, avec le nœud `agent_run` (D786). |
| Un SDK de graphe d'agent maison (style LangGraph) | Le bac exécute déjà n'importe quel harnais. Un graphe d'agent se code dans le bac. |
| Une gestion de prompts à part | Les compétences sont déjà versionnées, avec une activation (rôle d'une étiquette `production`). |
| Un RCA par LLM sans topologie | Le résultat est une histoire assurée, pas une preuve. |
| Les pourcentages de causes trouvées annoncés par les éditeurs | Les chiffres ne sont pas reproductibles. PneX mesure sur un jeu d'incidents étiquetés (critère McNemar, `cases.md` §3.6). |
| La fusion ou la remédiation autonome | Aucun leader ne le fait par défaut. PneX garde D838 et D850. |
| Un SIEM ou un store de logs de plus | PneX lit ceux du client (D1001). |
| La découverte active (scan réseau, eBPF) dans PneX | PneX lit les outils de découverte existants, et l'agent edge seulement sur demande (Q-IT3). |

## 6. Sources

- Paperclip : https://docs.paperclip.ing/ , https://github.com/paperclipai/paperclip , https://docs.paperclip.ing/guides/projects-workflow/routines
- Langfuse : https://langfuse.com/docs , https://langfuse.com/open-source
- LangChain : https://docs.langchain.com/ , https://docs.langchain.com/langsmith/managed-deep-agents-schedules
- Coroot : https://github.com/coroot/coroot · Keep : https://www.elastic.co/blog/elastic-and-keep-join-forces · HolmesGPT : https://www.cncf.io/projects/holmesgpt/ · Cartography : https://github.com/cartography-cncf/cartography · Grafana Knowledge Graph : https://grafana.com/docs/grafana-cloud/monitor-applications/asserts
- OpenHands : https://docs.openhands.dev/openhands/usage/automations/creating-automations · Copilot : https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/customize-the-agent-environment · Codex : https://developers.openai.com/codex/cloud/environments · Cursor : https://cursor.com/docs/cloud-agent/automations · Jules : https://jules.google/docs/scheduled-tasks/ · Claude Code routines : https://code.claude.com/docs/en/routines · Devin : https://docs.devin.ai/api-reference/release-notes
- API de lecture : https://www.elastic.co/docs/reference/elasticsearch/rest-apis/paginate-search-results , https://openobserve.ai/docs/reference/api/search/search.md

## 7. Fiches détaillées

Une fiche par projet dans `docs/journal/bench/agents-ops-correlation/`. Les dépôts sont clonés en `--depth 1` dans `~/Documents/shan-perso/bench/`, hors du workspace, et rien n'y est exécuté.

| Fiche | Commit | À reprendre pour les agents métier de PneX |
|---|---|---|
| `openhands.md` | SDK `af85e224`, automation `19c4ea68` | Reprise sans rejouer l'outil interrompu : il reçoit une erreur « interrompu », et le modèle décide (D1034). Un bail empêche deux serveurs de reprendre le même run. La condensation du contexte n'efface rien : le journal reste complet. Cinq motifs d'agent bloqué au lieu d'un seul. Un étage déterministe avant le validateur, car le risque déclaré par le modèle est manipulable. Des campagnes d'évaluation liées à une version exacte, reprises après un crash (D1033). |
| `paperclip.md` | `28aa32e7` | Un budget en argent, réservé avant le run et revérifié avant le premier appel. Un prix inconnu vaut « inconnu », pas « gratuit ». Une alerte à 80 %. Un arrêt dur suivi d'une demande de dépassement. Un frein par org sur les runs automatiques seulement (taux d'échec, refus du fournisseur), jamais sur un run lancé par une personne. Une file équitable : runs humains d'abord, un départ sur 4 au plus ancien. Un jeton de run qui prouve l'origine d'un run lancé par webhook. Une sonde du harnais avant d'épingler un environnement. Pièges : réveils périodiques coûteux, sessions qui grossissent sans fin, secret affiché dans un journal. |
| `langfuse.md` | `e9bbbb91` (v4.84.0) | Le mapping OTel GenAI vers des types d'étape (génération, agent, outil, garde-fou, évaluateur) est un modèle direct pour D1031. Les cas sont versionnés, et chaque résultat copie l'entrée et l'attendu du moment, donc un ancien run reste lisible (D1033). Le juge répond en sortie structurée : un score typé et sa justification. Contre la double dépense : échantillonnage déterministe par empreinte de la cible, et identifiant d'exécution dérivé de (règle, cible). Un juge ne juge jamais un autre juge. Un score « correction » humain peut alimenter la Table de cas. À écarter : le contenu stocké par défaut, un produit de prompts à part, la table de prix tenue à la main. |
| `langgraph.md` | langgraph `b4fe40f8`, deepagents `b04f2ef7` | Idempotence par identifiant déterministe : chaque appel d'outil reçoit l'empreinte de (run, étape, rang), avec une contrainte d'unicité sur les propositions (D1034). Sauver chaque étape avant la suivante, jamais en différé (bugs ouverts sur les modes asynchrones). La pause s'arrête à la frontière d'étape, et la décision nomme la proposition : leur `interrupt()` rejoue le nœud entier. Un même enregistrement en arbre sert à la reprise, à la trace (D1031) et au rejeu depuis l'étape N (D1033). Un résultat d'outil trop long est déporté dans un artefact avec un aperçu. Résumé du contexte à 85 %. Compétences montrées par leur nom et lues à la demande. À écarter : le moteur de graphe générique (les flows le couvrent), un store de mémoire à part de l'ontologie, qui contournerait l'IAM. |
| `coroot.md` | `6927e8f7` (v1.29.4) | Le code de cause racine n'est pas libre : la version gratuite envoie les métriques à leur cloud. Le principe confirme D1007 : le graphe et la comparaison trouvent, le LLM reçoit les constats et rédige. Un run d'analyse par incident, avec une fenêtre bornée (D1010). Une sortie structurée : résumé, cause, gestes, preuves, carte de propagation avec un état par lien (D1011). Le réseau est toujours imputé au client, jamais à un lien physique partagé : c'est le manque que comble la couche `network`. Identité d'application `cluster:namespace:kind:name`. Risques de disponibilité (instance unique, base non répliquée) à côté de l'exposition (IT5). Comparaison par version déployée. À écarter : l'agent eBPF, une topologie reconstruite à chaque requête et invisible sans trafic, un incident par application. |
| `keep.md` | `60aa8e05` (v0.55.0) | Projet en maintenance après le rachat. Dédoublonnage à deux niveaux : l'empreinte dit « même alerte », un hachage du contenu moins les champs ignorés dit « même contenu ». Une trace SUCCESS ou SKIPPED de chaque règle de rattachement, par signal (D1005, exigence 5). Champs de règle de corrélation à reprendre : clés de regroupement, fenêtre glissante, seuil, création sur un ou tous, clôture (`resolve_on`), approbation. Manquent à `it-operations.md` : la clôture d'un incident et les fenêtres de maintenance. La sortie du LLM est un JSON strict avec une confiance, approuvé par un humain (IT6). Les liens de service déduits des traces sont une source de découverte peu coûteuse (IT2). À écarter : la réécriture de la configuration des sources, la lecture sans curseur, une topologie à plat sans historique, l'envoi d'alertes brutes au LLM. Contre-exemple de sécurité : des dépendances sans identifiant de tenant (R1). |
| `holmesgpt.md` | `75ce939e` (0.47.0) | Au dernier pas, les outils sont retirés : le modèle doit conclure, et le run ne finit jamais sur un simple dépassement. Un appel répété est refusé dès la deuxième fois, comme une erreur d'outil, donc le modèle change d'approche. Un résultat d'outil est plafonné à 15 % du contexte, le reste va dans un artefact avec un aperçu. Les sources indisponibles sont citées au modèle avec leur erreur, pour qu'il dise « intégration absente » au lieu d'inventer. Le format d'impact en quatre groupes (touché avec preuve citée, vérifié sain, changé pour une autre raison, non vérifié) est une bonne sortie pour IT6 et D1008. Évaluation : environ 300 cas avec résultats attendus et tags, mesures par modèle (taux, coût, latence P50/P95, répétitions). Il faut aussi rejouer des sorties d'outil enregistrées (IT7). À écarter : la cause racine par LLM seul, un run par alerte, des requêtes de logs écrites par le LLM, l'alerte brute collée dans le prompt. |
| `cartography.md` | Cartography `adae46d2`, BloodHound `659e3caf` | Aucun historique : le nettoyage supprime tout ce qui n'a pas été vu au dernier passage. PneX fait l'inverse et ferme le lien (`valid_to`). Leur pire classe de bugs : une lecture partielle (403, 5xx) efface des actifs réels. D'où une exigence ajoutée à `it-operations.md` : ne jamais fermer de liens sur une liste incomplète. Un schéma déclaratif qui génère l'écriture et le nettoyage, borné au tenant. Une couche canonique (`ComputeInstance`, `Image`, `UserAccount`, `CVE`) au-dessus des types de chaque fournisseur. Le digest d'image relie le runtime à la chaîne logicielle (D1009, SD6). Règles d'exposition Règle → Fait → Constat, avec un actif d'ancrage et une clé stable entre deux passages (IT5). Aucune règle ne croise faille, exposition Internet et environnement : c'est le manque que PneX comble. BloodHound distingue les liens ingérés et les liens déduits, et marque les liens parcourables. Le format d'extension OpenGraph est une bonne forme de pack. À écarter : Neo4j, l'exposition posée en simple drapeau (le chemin et la date sont perdus), l'identité par e-mail seul. |
| `netbox.md` | NetBox `112a6282` (v4.7.5), Nautobot `115cb277` | Les chemins de câble sont précalculés : PneX lit l'extrémité connectée ou la route `trace`, sans refaire le calcul. Les panneaux de brassage se replient en un seul `network_link`. NetBox ne dit jamais quel bout est en amont : il faut orienter `connected_to` par une règle sur le rôle de l'équipement (cœur, distribution, accès, serveur), dans un flow (D998). Lecture initiale avec un curseur `start=<id>`, puis suivi par le journal des changements, car `last_updated` ne montre pas les suppressions. Ce journal est purgé après 90 jours. Les webhooks sont signés en HMAC-SHA512, sans horodatage, et servent seulement de réveil. La découverte comparée à la déclaration est payante chez NetBox Labs : c'est la valeur de PneX (D1004, Q-IT3). Réponses proposées : Q-IT2 « les deux » (lecteur `network_inventory` plus import CSV au format d'export NetBox), Q-IT3 « plus tard, en lecture seule ». À écarter : devenir un éditeur de baies et de câbles, écrire vers NetBox, importer chaque port vide. |
| `kagent.md` | `e8cd1533` | Le harnais du bac reçoit une clé factice. Une passerelle de sortie pose la vraie en sortant, pour une seule destination. PneX le fait déjà pour la clé LLM (D385), et pourrait l'étendre aux secrets d'API d'un run. Les demandes de permission d'un harnais tiers (Claude Code) deviennent la même pause d'approbation humaine : c'est une voie pour étendre la porte de validation aux agents hébergés. Pause humaine avec une décision par `call_id`. Une réponse incomplète, en double ou périmée est refusée, et un refus arrive au modèle avec sa raison (D1034). Un jeton court de l'agent, borné à des audiences nommées, pour appeler les services de l'org. Le contrat d'attributs OTel est un registre, et le contenu des messages n'est capté que sur option (D1031, D1032). Cron avec fuseau explicite, sans rattrapage. À écarter : les CRD comme source de vérité, un proxy qui déchiffre le TLS avec une CA dans chaque charge, des sessions longues en sommeil, une mémoire résumée par LLM, et l'autorisation ouverte par défaut. |
