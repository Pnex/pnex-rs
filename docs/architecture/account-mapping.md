# Cartographie commerciale des comptes (D329–D343)

| | |
|---|---|
| Statut | **Proposition — 2026-10-21** ; CC1 en cours (pack, voir §10) |
| Déclencheur | Besoin de prospection B2B : développer les comptes existants (filiales, sites, décideurs), cibler de nouvelles entreprises, repérer les événements déclencheurs, garder une trace de qui a recommandé qui |
| Repose sur | `ontology.md` (D176–D191), `partner-watch.md` (D300–D316 : imports, clé naturelle, pack `entreprises_fr`, vue Hiérarchie, crawl, preuves, nœuds ontologie), `extension-collector.md` (D254–D284), `cases-money.md` et `cases-graph-sigma.md` (graphe, chemins K51), `iam.md` (D298 classification) |
| Numérotation | **D329…D343**, lots **CC0…CC6** |
| Cas de référence | un compte client à quelques sites dans un groupe plus large ; un grand groupe international avec filiales étrangères |

## 1. Le besoin en une phrase

Voir un compte comme un **réseau daté** (groupe, filiales, établissements, dirigeants, contacts, recommandations), sur une carte et dans un graphe, savoir où on est déjà présent et où est le potentiel, et être prévenu quand quelque chose bouge dans ce réseau.

## 2. Non-objectifs

- **Pas de CRM** : ni pipeline d'opportunités, ni devis, ni séquences d'e-mails, ni comptes rendus de réunion. PNEX cartographie et signale ; le suivi commercial reste dans l'outil de vente.
- **Pas de connecteur CRM** dans ce PRD.
- **Pas de découverte automatique de personnes** (D254 inchangé) : aucune recherche de profils, aucun parcours de graphe social, aucun rafraîchissement automatique d'un profil de réseau social.
- **Pas de type « entreprise » ou « compte »** à côté de `organization` (même raison que D304 : un second type couperait le graphe en deux).
- Rien de propre à un vendeur ou à un secteur au noyau (D190) : tout ce qui est métier vit dans un pack.
- Rien de propre à une méthode de vente dans le pack non plus : profil de client idéal, personas visés, signaux déclencheurs, secteurs prioritaires et rôles de l'équipe commerciale sont des **réglages ou des données de l'org**. Le pack fournit la forme, jamais le contenu d'un playbook.

### 2.1 Ce que le pack doit pouvoir porter (relecture d'un playbook commercial réel, 2026-10-21)

Un playbook B2B industriel typique a servi de banc d'essai. Ce qui s'y trouve et où ça tombe :

| Élément du playbook | Où il vit | Générique ? |
|---|---|---|
| Profil de client idéal (secteurs, tranche d'effectif, CA, effectif **par site**) | critères de score de l'org (D336), au niveau société **et** site | oui : critères déclaratifs |
| Personas visés (décideur économique, champion, sponsor, utilisateur clé) | `persona` = rôle dans le comité d'achat ; l'intitulé de poste reste sur `employs.position` | oui : vocabulaire usuel des grilles de qualification |
| Signaux déclencheurs (montée en cadence, gros contrat, départs en retraite, problèmes qualité, déploiement multi-sites, école interne) | **thèmes** de signal définis par l'org, pas une liste du pack (D335) | oui : la liste fermée est un réglage |
| Rôles de l'équipe (prospection, closing, grands comptes, événementiel) | `suit_le_compte.role`, texte libre ; plusieurs suiveurs par compte | oui |
| Salons et groupements professionnels | liste d'exposants ou de participants importée avant l'événement (D337) ; groupement = `organization` + `member_of`, utilisable par le chemin d'introduction (D338) | oui |
| Concurrence identifiée sur un compte | `utilise_solution` | oui |
| Pipeline, cadence de relance, scripts, objections, devis | outil de vente | **hors périmètre** (pas de CRM) |
| Passation vers l'outil de vente | un signal `retenu` ou un plan de compte part par les sorties existantes (webhook HMAC, e-mail, export PDF) | oui, sans connecteur |

## 3. Ce qui existe et ce qui manque

| Besoin | Existant | Manque |
|---|---|---|
| Société, établissements, dirigeants | pack `entreprises_fr` : `organization` (SIREN unique, NAF, effectif), établissement = `map_pin` (SIRET), lien `dirige` daté, synchro API Recherche d'entreprises | chiffre d'affaires, LEI, pays |
| Groupe, filiales | vue Hiérarchie `groupe` (D305) sur `owns`, `etablissement_de`, `dirige` ; multi-parents, à une date | **liens mère-fille** : l'API Recherche d'entreprises ne donne ni participations ni filiales étrangères |
| Contacts | `person`, `employs` (poste), `identifier` (compte, e-mail…), `knows` (sans attribut) | persona, recommandation datée, compte suivi par qui |
| Statut commercial | rien | client / prospect / cible / ancien client / concurrent, utilisateurs actuels vs potentiel |
| Carte | `map_pin`, page Sites, carte MapLibre | coloration par statut |
| Graphe et chemins | Sigma.js, chemins entre deux objets (K51), expansion | chemin d'introduction restreint aux liens « humains » |
| Imports | D301 fichier / URL / API, aperçu, clé naturelle, `snapshot` qui ferme les absents | rapprochement nom de société → SIREN |
| Veille | crawl de site borné (D307), preuves (D310), diff de versions (D311), `llm_text` / `text_decide` | objet signal, règles de détection livrées par le pack |
| Profil d'une personne | mission `collect_page` (texte de la page, session de l'utilisateur) | mission ponctuelle, extraction vers l'ontologie |
| Données personnelles | minimisation D304 | classification D298 (non codée), opposition, rétention |

## 4. Décisions

### D329 — Pack `prospection_b2b` : étendre, ne pas créer

Pack générique (`requires: [entreprises_fr]`), sans valeur propre à un vendeur.

- `organization` étendue (D306) :
  - `statut_compte` (enum : `client`, `prospect`, `cible`, `ancien_client`, `concurrent`, `partenaire`, `exclu`),
  - `utilisateurs_actuels`, `potentiel_utilisateurs` (nombres),
  - `lei` (texte, unique), `pays` (ISO 3166-1),
  - `ca_dernier` (montant, propriété temporelle D178 : une valeur par exercice), `score_cible` (nombre, calculé, D336).
- `map_pin` étendu : `statut_site` (`client`, `prospect`, `non_couvert`), `utilisateurs_actuels`.
- `map_pin` gagne aussi `effectif_tranche` côté `entreprises_fr` (1.1.0) : le registre le publie par SIRET, ce qui permet de cibler un **site** et pas seulement une société.
- `person` étendue : `persona` = rôle dans le comité d'achat (enum : `decideur`, `champion`, `sponsor`, `prescripteur`, `acheteur`, `utilisateur`, `technique`, `autre`) — l'intitulé de poste reste sur `employs.position` ; `revoir_le` (date, D334).
- Nouveaux liens :
  - `recommande` (person → person | organization) : date, contexte, source. C'est le « qui a recommandé qui » ;
  - `suit_le_compte` (person interne → organization) : `role` (texte libre, rôle que l'org donne à ce suiveur) ; plusieurs suiveurs par compte, tous destinataires des signaux ;
  - `filiale_de` (organization → organization, une cible à une date) : `nature` (`consolidation_directe`, `consolidation_ultime`, `declaree`, `wikidata`, `saisie`), source, consulté le (D331) ;
  - `utilise_solution` (organization | map_pin → organization concurrente) : depuis, source.
- Hiérarchie `groupe_commercial` : `filiale_de` inverse, puis `owns`, puis `etablissement_de` inverse. La hiérarchie `groupe` d'`entreprises_fr` reste inchangée.

Pourquoi `filiale_de` plutôt que `owns` : les sources ouvertes donnent une relation de **contrôle ou de consolidation**, sans pourcentage. Mettre un `owns` sans part mentirait sur ce qu'on sait. `owns` reste réservé aux participations connues.

### D330 — La couverture d'un compte est calculée à la lecture

Sur la vue Hiérarchie (D305) et la carte, chaque nœud porte un **résumé calculé** :
- sites clients / sites connus ;
- utilisateurs actuels / potentiel ;
- nombre de contacts par persona ;
- nombre de signaux ouverts.

Rien n'est stocké en agrégat : c'est un calcul borné, fait à la lecture comme la hiérarchie (profondeur ≤ 6, 500 nœuds), à une date T. La vue « **espace blanc** » liste les filiales et établissements d'un groupe sans aucun `employs` ni `statut_site = client` : c'est la liste de prospection du compte.

### D331 — Liens de groupe depuis GLEIF et Wikidata

Deux mappings d'import API (D301, D303), livrés dans le pack, en mode `incremental` :

- **GLEIF** (registre public des LEI, gratuit) : rapprochement SIREN → LEI, puis relations société mère directe et ultime → `filiale_de` avec la nature `consolidation_directe` ou `consolidation_ultime`. Couvre les filiales étrangères des grands groupes. Une mère étrangère sans SIREN devient une `organization` de clé `lei`.
- **Wikidata** : propriétés SIREN, société mère et filiale → `filiale_de` de nature `wikidata`, toujours à l'état **« à valider »** (D316), parce que la donnée est communautaire.
- Conflit entre sources : on ne fusionne pas. Chaque lien garde sa provenance (D184), la fiche montre les deux et l'humain ferme celui qui est faux.
- PME sans LEI ni fiche Wikidata : saisie manuelle (`nature = saisie`) ou import d'un fichier.

### D332 — Rapprochement d'un nom de société vers un SIREN

Nouveau nœud `org_resolve` : entrée nom + ville (ou code postal) ; il interroge l'API Recherche d'entreprises et rend 0 à 5 candidats avec un score. Il n'écrit rien : au-dessus d'un seuil, il propose le rattachement (état « à valider ») ; en dessous, il laisse la ligne à un humain. Sert aux listes de salon (D337) et à la capture de profil (D333).

### D333 — Capture ponctuelle d'un profil dans la session de l'utilisateur

**Amende D261 et l'usage de `collect_page`** (aujourd'hui en rafraîchissement périodique) : la mission gagne un mode `once`.

- Déclenchement **humain uniquement** : bouton « Capturer le profil » sur la fiche `person` (l'utilisateur colle l'URL), ou « Envoyer cet onglet » depuis l'extension. Une URL, une capture. Jamais créée par un flow ni par l'assistant.
- L'agent ouvre l'URL dans la session déjà connectée du profil navigateur de l'utilisateur, au rythme D258, sans contournement (mur de connexion, captcha ou limite → `blocked`).
- Plafond par agent et par hôte de réseau social : **30 captures par jour** par défaut. Le plafond est compilé, l'org ne peut que le baisser.
- Interdit au noyau : les pages de résultats de recherche, « autres profils consultés », les listes de relations. Refus `ext-target-not-profile` sur ces motifs d'URL, liste livrée par adaptateur.
- Extraction : le texte arrive en document (D261) ; un flow du pack (`media_source` → `llm_text` avec un schéma de sortie fermé) propose nom, poste, société, URL du profil. On obtient alors `person` + `identifier` (`account`, l'URL) + `employs` (poste) + rattachement à l'`organization` via `org_resolve`, **à l'état « à valider »**.
- Minimisation : seuls les champs du schéma sont gardés. Le texte brut de la capture a une rétention courte (30 jours, réglable à la baisse) puis il est purgé.
- Rappel à la création de la mission (D254) : les conditions de la plateforme peuvent interdire l'accès automatisé ; le risque porte sur le compte de l'utilisateur.

> **Amendé et livré le 2026-10-21 (sans l'extraction)** : la capture est **générique par réseau** (LinkedIn, X, Facebook, Instagram, TikTok, YouTube) et **toujours rattachée à une `person`**. Elle prend la forme d'une mission `capture_profile` créée par le serveur seul : `POST /api/v1/extension/people/{id}/profile-captures`, exclue du MCP et de l'assistant (motif « outward »). La reconnaissance d'un profil (`browser_agent::profile_of`, partagée par le serveur et l'agent) rend une adresse canonique, et toute autre page d'un réseau connu reçoit `ext-target-not-profile`. À l'arrivée de la page : document `published_by` la personne, adresse = `identifier` `account` relié par `uses_identifier`, mission close. Un renvoi vers une page de connexion ou de vérification bloque la mission (`ext-login-required`, `ext-challenge`).
> Plafond : 30 captures par jour, **par org et par réseau** (l'agent n'est connu qu'à l'exécution), code `ext-profile-daily-cap`. Le texte capturé est purgé après 30 jours (pruner `profiles`). L'extraction par LLM (flow du pack, « à valider ») reste au lot suivant.
> **D341, minimum livré** : liste `prospection_oppositions` (empreintes `person:<uuid>` et adresses de profil canoniques, qui survivent à la personne). « Enregistrer son opposition » sur la fiche, confirmée deux fois et sans retour arrière dans l'UI, ferme les captures en attente et refuse les suivantes (`prospection-opposed`). La classification D298, la base légale affichée, la file de rétention à 36 mois et l'effacement restent à faire.

### D334 — Changement de poste : registre, recapture humaine et rappel

PNEX ne surveille pas un profil de réseau social. Le changement de poste est connu de trois façons :
1. **Registre** : un lien `dirige` fermé ou ouvert lors de la synchro `snapshot` (mandataires sociaux seulement) crée un signal `changement_dirigeant`.
2. **Recapture** : quand une capture D333 donne une autre société, le flow propose de fermer l'ancien `employs` (`valid_to`) et d'ouvrir le nouveau. L'historique reste visible.
3. **Rappel** : `revoir_le` sur une personne (défaut : 6 mois après la dernière capture, pour les personas `decideur` et `prescripteur`) crée une tâche pour le commercial qui suit le compte. C'est lui qui revérifie, à la main.

### D335 — Objet `signal_commercial` et règles de détection

Type du pack : `kind` = **comment** le signal a été détecté (`nouvel_etablissement`, `fermeture_etablissement`, `changement_dirigeant`, `annonce_legale`, `page_modifiee`, `changement_poste`, `autre`, liste fermée du pack), `theme` = **ce dont** il parle (liste de l'org, voir plus bas), `resume`, `etat` (`a_valider`, `retenu`, `ecarte`), lien `concerne` → organization | map_pin | person, provenance et extrait cité (D316).

Règles livrées en flows du pack (non déployés à l'installation, D123) :
- **nouvel établissement** : un SIRET apparu lors d'une synchro d'un compte suivi (`statut_compte` ∈ client, prospect, cible). C'est le signal « nouvelle usine » ;
- **annonces légales BODACC** (le mapping prévu en V2 par D304 est avancé ici) : création, modification, procédure collective, sur les SIREN suivis ;
- **pages actualités et carrières** des comptes cibles : crawl D307 + `text_decide` sur la **liste fermée de thèmes de l'org** (réglage du pack, chaque thème = clé + description pour le LLM ; exemples livrés : ouverture de site, investissement, recrutement massif, nouvelle ligne de production, départs en retraite, problème qualité, déploiement multi-sites, école interne). L'org ajoute, retire, renomme ; le flow ne connaît que la liste ;
- **pages des concurrents** : diff D311 sur les pages offres et tarifs.

Notification au `suit_le_compte` du compte concerné (canal de l'org, D313 pour le destinataire). Un signal issu d'un LLM est toujours `a_valider` (D316).

### D336 — Score de ciblage : une fonction du pack, pas un modèle

Fonction Starlark du pack (D316), versionnée et générique : elle ne contient **aucun critère**. Les critères sont une liste déclarative de l'org (propriété, condition, points ; ex. NAF commençant par une liste de préfixes, tranche d'effectif ≥ un seuil, `ca_dernier` ≥ un montant, distance au site client le plus proche, signaux retenus sur 12 mois par thème). Le score se calcule pour une `organization` et pour un `map_pin` (effectif du site). Pas de ML : le score s'explique ligne par ligne sur la fiche (critère → points).

### D337 — Listes de ciblage et listes de salon

- **Ciblage** : un import API par recherche enregistrée (URL Recherche d'entreprises avec ses filtres : NAF, effectif, département), mode `incremental`. Les nouveaux SIREN entrent en `statut_compte = cible`, jamais en écrasant un statut existant.
- **Avant un salon** : la liste des exposants (fichier ou page) passe par `org_resolve` puis par le score : la liste de cibles à voir sur place sort de l'explorateur, triée.
- **Salon ou groupement** : mapping Excel livré dans le pack (participant, poste, société). Chaque ligne crée une `person` + `employs` ; la société passe par `org_resolve` (D332). La liste est rattachée à un objet `evenement` (type `place` + date, ou dossier) pour garder « rencontré au salon X ». Le tri par persona et par score se fait dans l'explorateur.

### D338 — Chemin d'introduction

Variante de K51 (chemins entre deux objets) restreinte aux liens `knows`, `recommande`, `employs`, `dirige`, `member_of`, profondeur ≤ 4. Point de départ : les personnes internes et les contacts des comptes `client`. Question type : « par qui puis-je être présenté au directeur industriel d'un site du groupe ? ». Le résultat se dessine dans le graphe Sigma.js avec le chemin surligné.

### D339 — Coloration par propriété (noyau, générique)

La carte, la vue Hiérarchie et le graphe acceptent une **règle de style par valeur de propriété** (couleur, icône, opacité), déclarée dans un pack et surchargeable par l'org. C'est un besoin générique (statut de site, criticité GMAO…). Ici : `statut_site` et `statut_compte`.

### D340 — Modèles de pages : plan de compte, fiche concurrent

Deux `page_templates` du pack, remplis par références d'objets (D187) :
- **plan de compte** : groupe, couverture (D330), espace blanc, décideurs par persona, recommandations, signaux retenus, prochaines actions (texte libre) ;
- **fiche concurrent** : offre, références connues (`utilise_solution`), dernières modifications de leurs pages (D311), arguments de défense (texte libre).

Export PDF par `page_pdf`.

### D341 — Données personnelles : classification, opposition, rétention

Préalable au lot CC3 (pas de capture de profil avant) :
- **D298 codée** pour les types du pack : nom, poste, URL de profil, e-mail → `personnelle`. Un rôle peut les masquer.
- **Base légale** affichée sur la fiche (`interet_legitime_prospection_b2b`), réglage de l'org, reprise dans l'export du registre des traitements.
- **Liste d'opposition** : à la suppression d'une personne sur demande, une empreinte (URL de profil normalisée, et nom + société) entre dans la liste. Imports, captures et rapprochements la consultent et refusent de recréer la fiche (`prospection-opposed`).
- **Rétention** : une personne sans capture, sans modification ni lien ouvert depuis 36 mois (réglable) est proposée à l'effacement dans une file. L'effacement est une action humaine.
- **Effacement** : suppression des propriétés `personnelle` et des identifiants ; il reste une coquille sans données, pour que les liens et la provenance des autres objets tiennent (voir Q-CC4).

### D342 — L'assistant et le MCP sans outil nouveau

Pas d'outil d'assistant propre au pack : `query_ontology`, `get_object`, la hiérarchie et les chemins suffisent ; le pack livre seulement des **exemples de questions** dans sa description. Le serveur MCP expose déjà ces opérations. L'assistant peut proposer un signal ou un rattachement (état « à valider »). Il ne peut ni lancer une capture (D333), ni modifier un `statut_compte`.

### D343 — Un socle mondial, un pack par pays (décision du 2026-10-21)

Le concept vaut pour tout pays ; seuls les identifiants, les codes locaux et les registres changent.

- **Pack `entreprises`** (mondial, `requires: [investigation]`) : `pays` (ISO 3166-1), `lei`, `code_activite` (classe **NACE rév. 2**, base européenne des codes nationaux), `effectif_min` / `effectif_max` (nombres, pas les tranches d'un pays), état, dates ; sur `map_pin` : siège, état, enseigne, effectif ; clé au registre des dirigeants ; liens `etablissement_de`, `dirige` ; hiérarchie `groupe`.
- **Un pack par pays** (`requires: [entreprises]`) : sa clé (SIREN/SIRET pour la France), ses codes locaux (NAF, tranche INSEE), son import de registre, qui **remplit aussi les champs communs** (NAF sans sa lettre → NACE ; tranche → effectif min/max ; `pays = FR`).
- **`prospection_b2b` dépend du socle**, pas d'un pays. Les critères de score portent sur les champs communs.
- **Pays sans pack** : GLEIF et Wikidata (D331), saisie, import de fichier.
- **Ordre** : France d'abord (`entreprises_fr` 2.0.0) ; ensuite, à confirmer au spike CC0, Royaume-Uni (registre gratuit avec dirigeants et bénéficiaires), Norvège, Danemark, Belgique. Un agrégateur mondial sous licence à partage à l'identique n'est jamais la source par défaut.
- **Mise à jour sans casse** : une clé (propriété ou type de lien) passe d'un pack pays au socle quand la version livrée du pack pays dépend du socle (`hands_over`). Une org en `entreprises_fr` 1.0.0 passe en 2.0.0 sans perte.

## 5. Lots

| Lot | Décisions | Sortie | Cas de référence |
|---|---|---|---|
| CC0 | — | Spike sources : ce que GLEIF, Wikidata et l'API Recherche d'entreprises (CA, filtres) renvoient vraiment pour les deux comptes de référence ; décision go/no-go par source | arbre de groupe attendu vs obtenu |
| CC1 | D329, D330, D339, D343 | pack installable, statut, couverture, espace blanc, coloration | carte du compte de référence : sites clients sur N, potentiel visible |
| CC2 | D331, D332 | liens de groupe GLEIF + Wikidata, `org_resolve` | hiérarchie du grand groupe avec filiales étrangères |
| CC3 | D341 puis D333, D334 | données personnelles, puis capture ponctuelle de profil + extraction à valider + rappels | 10 décideurs du compte de référence rattachés à leurs sites |
| CC4 | D335 | signaux et leurs flows (établissement, BODACC, pages, concurrents) | premier signal « nouvel établissement » sur un compte suivi |
| CC5 | D336, D337, D338 | score, listes de ciblage et de salon, chemin d'introduction | liste d'un salon passée en personnes rattachées et triées |
| CC6 | D340, D342 | plan de compte, fiche concurrent, exemples de questions | plan de compte du compte de référence exporté en PDF |

CC1 et CC2 ne touchent aucune donnée personnelle hors dirigeants (déjà couverts par D304). **CC3 attend D341.**

## 6. Sécurité et conformité

- **Sortie réseau** : GLEIF, Wikidata, BODACC et Recherche d'entreprises passent la garde `http_fetch` (R8).
- **Réseaux sociaux** : capture uniquement humaine, une URL de profil à la fois, plafond compilé, aucun contournement (D333). Les conditions de la plateforme restent la responsabilité de l'utilisateur ; l'avertissement D254 est affiché à chaque mission.
- **RGPD** : intérêt légitime pour la prospection B2B, minimisation (champs du schéma seulement), information de la personne au premier contact (hors PNEX), opposition et rétention (D341). Le texte brut capturé est purgé après 30 jours.
- **LLM** : un fait extrait n'est jamais confirmé sans humain (D316). Le texte capturé est traité comme injection potentielle (règle existante de l'assistant).
- **Org** : frontière inchangée (R1) ; test viewer → 403 sur toute nouvelle route d'écriture (R2).

## 7. Mesures de succès

- Carte et hiérarchie d'un compte de référence construites en moins d'une heure, sources citées sur chaque lien.
- Part des établissements d'un compte client ayant au moins un contact identifié.
- Nombre de signaux `retenu` par mois, et part des signaux `ecarte` (qualité des règles).
- Zéro capture de profil non déclenchée par un humain (contrôle dans le journal des missions).

## 8. Questions ouvertes

1. **Q-CC1 — GLEIF suffisant ?** Les relations de consolidation couvrent les grands groupes ; pour les ETI sans LEI, faut-il une source payante en option (mapping livré mais désactivé) ?
2. **Q-CC2 — Personne interne** : faut-il un type « membre de l'équipe » relié aux utilisateurs PNEX, ou une simple propriété `interne` sur `person` ?
3. **Q-CC3 — `evenement`** : réutiliser `place` + date, un dossier (`cases`), ou un type de pack dédié pour les salons ?
4. **Q-CC4 — Effacement et provenance** : la coquille sans données (D341) satisfait-elle le droit à l'effacement, ou faut-il aussi réécrire les extraits cités dans les preuves et les signaux ?
5. **Q-CC5 — Plafond D333** : 30 captures par jour et par agent est-il le bon ordre de grandeur pour un usage commercial réel ?
6. **Q-CC6 — Valeurs d'enum de l'org** : à la mise à jour d'un pack, le pack gagne sur ce qu'il déclare ; une valeur de `persona` ajoutée par l'org serait perdue. Faut-il que l'org puisse étendre une enum du pack, ou passer `persona` en taxonomie ?

## 9. Récapitulatif des décisions

| # | Décision | Choix |
|---|---|---|
| D329 | Pack `prospection_b2b` | étend `organization`, `map_pin` (+ effectif par site), `person` (persona = rôle d'achat) ; liens `recommande`, `suit_le_compte`, `filiale_de`, `utilise_solution` ; hiérarchie `groupe_commercial` |
| D330 | Couverture | calculée à la lecture sur la hiérarchie, vue espace blanc |
| D331 | Liens de groupe | GLEIF (consolidation) + Wikidata (à valider), provenance par source, pas de fusion |
| D332 | Rapprochement | nœud `org_resolve`, candidats scorés, n'écrit rien |
| D333 | Profil | `collect_page` mode `once`, déclenché par un humain, session de l'utilisateur, plafond compilé, extraction à valider |
| D334 | Changement de poste | registre + recapture humaine + rappel `revoir_le`, jamais de surveillance automatique |
| D335 | Signaux | type `signal_commercial` (détection × thème de l'org), flows du pack, notification aux suiveurs, LLM à valider |
| D336 | Score | fonction Starlark sans critère en dur, critères déclaratifs de l'org, société et site |
| D337 | Listes | recherche enregistrée en import API ; salon en Excel + `org_resolve` |
| D338 | Introduction | K51 restreint aux liens humains, profondeur ≤ 4 |
| D339 | Style | coloration par valeur de propriété, au noyau |
| D340 | Pages | plan de compte, fiche concurrent |
| D341 | Données personnelles | D298 codée, opposition, rétention 36 mois, effacement en coquille |
| D342 | Assistant | aucun outil nouveau, pas de capture ni de changement de statut |
| D343 | Monde | socle `entreprises` (NACE, effectif chiffré, LEI, pays) + un pack par pays, France d'abord ; passation des clés à la mise à jour |

## 10. État d'implémentation

### CC1 — pack `prospection_b2b` (2026-10-21, non commité)

Livré : `crates/pnex-backend/packs/prospection_b2b.yaml` (D329), inscrit
dans la liste des packs livrés ; les gardes existantes de validité des
packs le couvrent.

Choix faits en route :
- **`ca_dernier` n'est pas temporel** : au noyau, une propriété temporelle
  (D178) est une série O2 ou un flux d'événements, pas une valeur par
  exercice. Le pack porte `ca_dernier` (`money`) + `ca_exercice` (année).
  L'historique par exercice reste visible dans l'historique des versions de
  l'objet. À revoir si CC0 montre qu'une source donne plusieurs exercices.
- **`filiale_de` sans `one_target`** : D331 garde côte à côte une mère
  directe et une mère ultime (GLEIF) et des sources en conflit
  (GLEIF / Wikidata) sans les fusionner ; une cible unique contredirait
  cette règle. La hiérarchie montre alors le nœud sous ses deux parents
  (`also_under`, D305).

Relecture de généricité (2026-10-21) : `persona` devient le rôle dans
le comité d'achat (+ `champion`, `sponsor`), `suit_le_compte` porte un
`role` libre, `entreprises_fr` 1.1.0 importe la tranche d'effectif de
chaque établissement. Constat CC0 en passant : l'API Recherche
d'entreprises renvoie `finances` (CA et résultat net par exercice) quand
la société publie ses comptes ; le mapping de `ca_dernier` en dépend.

Socle mondial (D343, 2026-10-21) : `packs/entreprises.yaml` créé ;
`entreprises_fr` 2.0.0 ne garde que SIREN, SIRET, NAF, tranche INSEE,
NDA, Qualiopi et l'import, qui remplit `pays`, `code_activite` et
l'effectif min/max ; `prospection_b2b` dépend du socle (`lei` et `pays` y
sont passés). Passation des clés à la mise à jour couverte par un test
(`country_pack_hands_shared_keys_to_the_worldwide_pack`). Limite : les
codes NAF d'avant la rév. 2 donnent une classe NACE fausse (rare au
registre actif).

Restent pour CC1 : couverture calculée et espace blanc (D330), coloration
par valeur de propriété (D339).

### CC0 — spike des sources (2026-10-22)

Appels réels sur Safran (562082909) et Mecachrome France (429853351) :

- **API Recherche d'entreprises** : aucune structure de groupe (ni mère ni
  filiales) ; les seules personnes morales dirigeantes sont les commissaires
  aux comptes. Elle donne CA (`finances`), tranche par SIREN et par site.
  Recherche par nom bruitée (« safran » : 2 010 résultats) : D332 est un
  préalable. Filtres de ciblage utilisables (`activite_principale`,
  `tranche_effectif_salarie`, `section_activite_principale`) ; plafond
  `page × per_page ≤ 10 000`. Ordres de grandeur : 8 codes NAF visés ×
  ≥ 100 salariés = 330 unités légales ; section C × ≥ 100 = 4 353.
- **GLEIF** : très bon sur une ETI (Mecachrome : mère MECA DEV, filiale
  canadienne), presque aveugle sur un grand groupe (Safran : 7 filiales
  directes déclarées ; Safran Aircraft Engines ne déclare aucune mère).
  `registeredAs` contient parfois des espaces.
- **Wikidata** : Safran = 20 filiales, dont 12 avec SIREN ; donne aussi
  l'Agence des participations de l'État comme mère, d'où l'état « à
  valider ».
- Verdict : GLEIF **et** Wikidata, côte à côte (D331 confirmé) ; saisie
  manuelle pour le reste. Le stock SIRENE complet est inutile pour un ICP :
  l'API suffit (D337).

### CC2 — liens de groupe et rapprochement (2026-10-22, non commité)

- **`filiale_de` passe dans `entreprises` 1.1.0** (la structure de groupe
  n'est pas propre à la vente), avec un attribut `etat` (`a_valider`,
  `confirme`) ; `prospection_b2b` 1.1.0 ne la déclare plus (passation par
  `hands_over`).
- **`entreprises_fr` 2.1.0** livre trois imports API par objet :
  - *Identifiants d'entité juridique (GLEIF)* : par SIREN, pose le LEI et
    relie aux mères directe et ultime **déjà connues** par leur LEI
    (`consolidation_directe` / `consolidation_ultime`, `confirme`) ; mère
    directe = ultime → un seul lien ;
  - *Sociétés mères (GLEIF)* : par LEI (`filter[owns]`), crée les mères
    nommées (françaises par SIREN, étrangères par LEI + pays) ; le tour
    suivant du premier import pose le lien ;
  - *Mères et filiales (Wikidata)* : par SIREN (SPARQL, P1616 + P749/P355
    dans les deux sens), seules les sociétés qui ont un SIREN, lien
    `wikidata` toujours `a_valider`.
- **Moteur d'import** (générique) :
  - `per_object` sans `param` : la valeur remplace `{value}` dans l'URL
    (chemin, texte SPARQL) ; seules les valeurs `[A-Za-z0-9._-]` y entrent ;
  - **tour de rôle** : les objets sont pris par ordre de valeur à partir du
    dernier interrogé par un run planifié ou manuel
    (`object_imports.per_object_cursor`, migration 000045), puis on reboucle.
    Avant, l'ordre « moins récemment modifié » rejouait sans fin les mêmes
    200 SIREN quand la source ne connaissait pas la plupart ;
  - un lot par objet vide n'est plus refusé comme « source perdue » ;
  - `when` accepte `not: true`.
- **Nœud `org_resolve`** (D332) : nom + ville / code postal → 0 à 5
  candidats notés (nom 70, même code postal 20 ou même ville 15, active 10 ;
  un nom seul plafonne à 80). Ports : *trouvée* (≥ seuil 80 et 10 points
  d'avance), *à revoir*, *échec*. Un code postal sans résultat est relancé
  sans lui. N'écrit rien ; plafond 2 000 appels par jour et par org
  (`settings.org_resolve.daily_calls`) ; registre surchargeable par
  `PNEX_COMPANY_SEARCH_URL` (tests). Fiche `assistant-kb/company-groups.md`.
- Tests : `partner_watch::group_links_from_gleif_and_wikidata` (Mecachrome
  par GLEIF, Safran par Wikidata, tour de rôle et rebouclage),
  `org_resolve::node_scores_register_candidates`, unitaires du score.

Limites :
- Le curseur n'avance pas sur un aperçu suivi d'une application : seul un
  run planifié ou « Lancer maintenant » le fait avancer.
- Une mère créée par Wikidata ou GLEIF n'a que son nom, son SIREN ou son LEI
  et son pays ; l'import du registre l'enrichit à son tour.
- `org_resolve` n'est pas encore branché sur les listes de salon (D337).

### CC5, première partie — listes de ciblage et score (2026-10-22, non commité)

- **Pack `prospection_fr` 1.0.0** (`requires: [entreprises_fr, prospection_b2b]`) :
  le croisement pays × prospection (D343) ; ni `entreprises_fr` ni
  `prospection_b2b` n'a à connaître l'autre. Il porte l'import **Liste de
  ciblage** (D337) : une recherche de l'API Recherche d'entreprises
  paginée (`page_param: page`, 25 par page, 400 pages = le plafond de
  10 000 de l'API). URL livrée = exemple (section C, ≥ 100 salariés,
  actives) que l'org remplace par son ICP. Écrit l'unité légale (tranche,
  effectif min/max, NACE, dernier CA publié) et son siège ; les autres
  établissements viennent de l'import du registre, société par société
  (l'API ne renvoie aucun établissement sans `q`).
- **Moteur d'import** :
  - `defaults` sur une entrée d'objet : constantes écrites seulement quand
    l'import **crée** l'objet. C'est ce qui fait entrer un nouveau SIREN en
    `statut_compte = cible` sans jamais toucher le statut d'un compte connu ;
  - segment de chemin `@last` : la plus grande clé d'un objet ou le dernier
    élément d'un tableau (`finances.@last.ca` = CA du dernier exercice
    publié).
- **Score de ciblage (D336)** dans `prospection_b2b` 1.1.0 :
  - type `critere_ciblage` (propriété, opérateur `egal` / `parmi` /
    `commence_par` / `au_moins` / `au_plus` / `renseigne`, valeur, points,
    actif) : les critères sont des **objets de l'org**, le pack n'en livre
    aucun ;
  - fonctions Starlark `pb_criteres` et `pb_score` ; `organization` gagne
    `score_detail` (une ligne par critère rempli) et `score_le` ;
  - modèle de flow « Prospection · target score », non déployé à
    l'installation : toutes les 15 minutes, jusqu'à 500 organisations non
    notées du jour (`score_le ne $today`) ;
  - test : `pnex-node-starlark/tests/pack_rules.rs`.
- Limites :
  - pas de score par site : `map_pin` est un type système qu'un flow
    n'écrit pas ; on filtre les sites sur leur effectif dans l'explorateur ;
  - 600 écritures par minute et par org (garde des nœuds ontologie) :
    4 300 comptes demandent environ 2 h de tours de 15 minutes ;
  - `ca_exercice` n'est pas rempli par la liste (l'année reste une clé de
    l'API) ;
  - listes de salon (fichier + `org_resolve`) : pas encore faites.

### CC1, fin — couleurs (D339) et couverture (D330) (2026-10-22, non commité)

- **D339, noyau générique** : `PropertyDef.colors` (valeur d'énumération →
  jeton de la palette fermée `green`, `amber`, `red`, `blue`, `violet`,
  `gray`, `schema::COLORS`), refusé sur une autre sorte de propriété ou
  pour une valeur inconnue. La couleur d'un objet est celle de sa première
  propriété colorée renseignée (`objects::color_of`) :
  - **Hiérarchie** : `HierarchyNode.marks` (pastille et valeur) ;
  - **Graphe** de l'objet : `Neighborhood.colors` (contour du nœud) ;
  - **Carte** : `color` sur les points isolés de `/pois/cluster` (anneau et
    bord de l'étiquette dans `map.js`). Les grappes restent neutres.
  `prospection_b2b` colore `statut_compte` et `statut_site`. La surcharge
  par l'org n'est pas faite : elle modifierait une propriété qui
  appartient au pack.
- **D330, couverture lue à la volée** : une hiérarchie de pack peut
  déclarer `sums` (propriétés nombre additionnées) et `coverage` (liens qui
  couvrent, valeurs qui couvrent). `HierarchyView` rend `counts` (nœuds
  distincts par valeur colorée), `totals`, `white_space` (nombre) et
  `white_space` sur chaque nœud non couvert, racine exclue. `groupe_commercial` :
  utilisateurs actuels et potentiels, couvert par `employs` ou un statut
  `client`. Bandeau au-dessus de l'arbre, case « Espace blanc seulement ».
  Mot ajouté au lexique (garde : « zone blanche » refusé).
- Restent de D330 : contacts par persona et signaux ouverts (le type
  signal arrive avec CC4).
- Test : `partner_watch::group_coverage_and_status_colours` ;
  unitaires `colours_name_enum_values_from_the_palette`,
  `last_segment_takes_the_latest_key_or_element`.

