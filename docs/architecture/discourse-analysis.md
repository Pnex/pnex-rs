# PRD — Analyse de discours public : formules récurrentes, pluralisme, coordination

| | |
|---|---|
| Statut | **Proposé — 2026-10-12**, zéro code. Emplacement du composant **ouvert** (§5) |
| Contexte | Prolongement de l'enquête média (`cases-workspace.md` K42–K47) : repérer les formules qui reviennent d'un locuteur et d'un média à l'autre, mesurer la diversité des voix et des sources, documenter des indices de coordination |
| Repose sur | Captation et transcription (`media-ingest.md` D159–D175, lots 0–6 codés), déclarations et détecteurs (`cases-workspace.md` K43–K45), apparitions (`cases.md` K17), ontologie (liens temporels D179, packs D190), recherche documentaire (`doc-search.md` : Postgres + `pg_trgm` + `pgvector`), assistant (`ai-assistant.md` §9, D123) |
| Numérotation | **DA1…DA10** |

## 1. Le besoin

Sur un corpus d'émissions captées, de documents et de jeux de données
importés, on veut :
- retrouver les formules qui reviennent mot pour mot chez plusieurs
  locuteurs ou dans plusieurs médias, et savoir où elles apparaissent en
  premier ;
- mettre ces propos en regard de faits structurels sourcés : qui détient
  quel média, qui intervient où et combien, quels liens unissent les
  intervenants aux sujets dont ils parlent ;
- documenter des indices de coordination, sans conclure à la place de
  la personne qui enquête.

Trois niveaux, tenus séparés dans tout le produit :

| Niveau | Exemples | Ce que fait PNEX |
|---|---|---|
| **Faits mesurables** | Composition de l'actionnariat d'un média à une date ; temps d'antenne par intervenant et par chaîne ; une même formule prononcée par six personnes en trois jours ; un intervenant salarié d'une entreprise de 2019 à 2022 | Les calcule et les affiche, chacun ouvrable à sa source |
| **Indices** | Apparition synchronisée d'une formule ; antériorité d'une formule dans une source donnée ; affiliation non mentionnée à l'antenne ; forte concentration des intervenants | Les **propose**, avec le calcul, les seuils et les sources ; l'humain retient ou écarte avec un motif |
| **Interprétations** | Qualification d'une campagne, d'une stratégie ou d'une intention | **Jamais produites par l'outil.** Elles relèvent de l'analyste, qui peut les rédiger dans une page de l'enquête, sous sa signature |

## 2. Formules récurrentes

### DA1 — Une formule récurrente est un objet, ses occurrences sont des déclarations

- Type `talking_point` (pack `media_watch`, §8) : `phrase` (forme
  canonique), `first_seen` (`fuzzy_time`), `status` (`proposed`,
  `retained`, `rejected`).
- Lien `repeats` : `statement` → `talking_point`, attribut `similarity`
  (`verbatim` | `variant`). Une variante (même idée, autres mots) n'est
  jamais présentée comme une reprise mot pour mot.
- Sur la frise de l'enquête, une ligne « Formule » montre la première
  occurrence puis la diffusion (qui, quel média, dans quel ordre).
  Chaque occurrence s'ouvre à la seconde.

### DA2 — Détection mot pour mot : déterministe et vérifiable

- Le texte transcrit est normalisé (casse, ponctuation, élisions), puis
  découpé en fragments de *k* mots (k = 6 à 8, à mesurer).
- On ne garde qu'une empreinte par fenêtre de *w* fragments (sélection
  par minimum, dite *winnowing*, classique en détection de recopie). Le
  volume baisse d'un facteur ≈ w, et deux textes qui partagent un passage
  assez long partagent forcément une empreinte.
- Les tournures courantes sont écartées par leur fréquence dans le
  corpus de référence de l'org (formules de politesse, transitions
  d'antenne).
- Une **formule candidate** est une empreinte retrouvée chez ≥ *n*
  locuteurs distincts ou dans ≥ *m* médias distincts sur une fenêtre *T*.
  Défauts à mesurer : 3 locuteurs, 2 médias, 7 jours. Elle est proposée
  avec ses occurrences, chacune vérifiée mot pour mot dans sa
  transcription (même garde que K44).

### DA3 — Variantes : rapprochement proposé par l'assistant

- Recherche par similarité de sens (`pgvector`, déjà en place pour les
  documents) sur les déclarations, autour d'une formule retenue. Le LLM
  de l'org, en classifieur fermé (`same_idea | different`), confirme.
- Le résultat est une proposition `similarity = variant`, marquée
  « rapprochement proposé par l'assistant ».

## 3. Mesures de pluralisme et de contexte

Des mesures sourcées, que le modèle porte presque entièrement
aujourd'hui :

| Thème | Ce qu'on mesure | Où sont les données |
|---|---|---|
| Actionnariat des médias | Graphe à date des liens temporels « possède » (actionnaire → groupe → chaîne) | Ontologie, pack Couverture médiatique (D190) |
| Présence à l'antenne | Temps d'antenne et nombre de passages par intervenant, par chaîne, par groupe, par période | Apparitions (K17) × plages d'émission (D169), agrégat D182 |
| Affiliations des intervenants | Intervenant présenté sous un titre (« économiste ») alors qu'un lien sourcé le rattache à une organisation concernée par le sujet traité | Ontologie (`employs`, `member_of`, `owns`) × déclaration et son sujet |
| Parcours professionnels | Chronologie des liens « employé par » d'un intervenant | Liens temporels (D179) |
| Diversité | Diversité des intervenants et des sujets par chaîne et par groupe ; traitement d'un sujet selon le groupe | Agrégats sur apparitions et sujets (D168) |

### DA4 — Des agrégats et des propositions, pas de scores de personnes

- Les mesures sont des **agrégats** (par chaîne, par groupe, par période,
  par sujet) ou des **propositions** sourcées (« affiliation non
  mentionnée »). Aucun score n'est attaché à une personne : un chiffre
  unique sur un nom résume mal et se lit comme un jugement.
- La proposition « affiliation non mentionnée » montre le titre annoncé
  à l'antenne (grille ou transcription), le lien de l'ontologie et sa
  source, et le sujet de la déclaration. L'humain retient ou écarte
  (motif : « affiliation sans rapport avec le sujet », etc.).

## 4. Indices de coordination

### DA5 — Ce que PNEX documente, et ce qu'il laisse à l'analyste

Les cadres méthodologiques publics d'analyse de la manipulation de
l'information convergent sur quelques familles d'indices. PNEX peut en
documenter certaines :

| Indice | Comment | Statut |
|---|---|---|
| **Synchronisation** : même formule, dans des médias sans lien apparent, sur une fenêtre courte | DA2 avec l'antériorité : qui en premier, à quel intervalle, chez qui | Proposé |
| **Antériorité dans une source qualifiée** : la formule apparaît d'abord dans une source porteuse d'une étiquette documentée (par exemple un média dont l'actionnariat ou le financement est public), puis est reprise | Étiquette portée par l'objet média, avec sa source ; DA2 sur l'ensemble | Proposé |
| **Liens** : financement, actionnariat, partenariat, emploi | Liens sourcés de l'ontologie, graphe à date | Fait sourcé |
| **Écart avec les faits** | Écart avec un fait sourcé (K44) | Proposé |
| **Intention, commanditaire** | — | **Hors outil** : interprétation de l'analyste |

### DA6 — Périmètre des données

- La collecte automatisée sur les réseaux sociaux et le moissonnage de
  profils sont un **non-objectif** (`cases.md` §4) : PNEX n'a aucun
  collecteur de personnes.
- PNEX couvre les flux audio et vidéo **déclarés**, les documents, et
  les **jeux de données importés** avec leur provenance (par exemple un
  export fourni par une équipe de recherche).
- Étendre ce périmètre serait une **décision produit et juridique** à
  part, non prise ici. Proposée le 2026-10-18 : `extension-collector.md`
  D254 (sources publiques déclarées une par une, agents navigateur).

## 5. Où vit le composant (ouvert)

Aujourd'hui, seule la captation existe. Trois emplacements possibles,
qui ne s'excluent pas :

| Option | Description | Pour | Contre |
|---|---|---|---|
| **A — À la demande, par l'assistant depuis une note** | Dans une page de l'enquête, on écrit une consigne : « cherche les formules récurrentes sur ce sujet entre le 3 et le 10 mars, sur ces 4 chaînes ». L'assistant appelle un **outil de lecture** `find_repeated_phrases` (calcul DA2 déterministe, en Rust, sur les transcriptions O2 de la fenêtre et des flux demandés), puis place ses propositions dans la file de revue | Aucune infrastructure permanente ; démarre avec la captation actuelle ; calcul borné par la demande ; résultat déjà rattaché à l'enquête | Calcul à chaque demande (relecture d'O2) ; une formule n'apparaît que si on la cherche |
| **B — Au fil de l'eau, dans le worker de transcription** | Chaque segment transcrit produit ses empreintes (DA2), stockées dans Postgres (`phrase_fingerprints` : empreinte, flux, instant, segment ; index B-tree, fenêtre glissante) | « Où d'autre cette phrase a-t-elle été dite ? » devient instantané ; une formule émergente peut être signalée | Volume à tenir (≈ 20 000 lignes par jour et par flux, ordre de grandeur à mesurer) ; il faut choisir les flux indexés |
| **C — Un flow de veille** | Un nœud `phrase_watch` dans un flow déployé par l'humain (D123), qui suit des formules ou des intervenants et alerte | S'intègre aux notifications existantes | Le runtime de flows n'a pas encore d'accès au LLM de l'org (`media-ingest.md`, lot 2) ; veille seulement, pas de découverte |

**Proposition** : commencer par **A**, qui démarre sans infrastructure et
valide l'usage. Le calcul DA2 vit dans `pnex-core` (fonction pure,
testée) pour être repris tel quel par **B** si le volume et l'usage le
justifient. **C** viendra plus tard, en s'appuyant sur B.

Dans A, l'outil respecte la règle §9.3 de `ai-assistant.md` :
- lecture seule, org du principal (R1) ;
- flux désignés par leur id dans l'org, jamais un nom de stream fourni
  par le modèle ;
- fenêtre et nombre de flux bornés ;
- résultat = propositions en file de revue, aucun objet créé d'office ;
- fiche `assistant-kb` et test « registre == ensemble autorisé ».

Le même chemin sert à l'extraction depuis une transcription ou une note
(personnes, organisations, déclarations, sujets) : c'est la file de revue
de `cases.md` K11, l'humain accepte un par un ou par lot.

## 6. Architecture des données

| Couche | Où | Rôle |
|---|---|---|
| Brut | O2 `tx_<slug>` (existant, D165) | Tout ce qui est capté ; relu par fenêtre de temps et par flux ; `match_all` pour une formule **connue**, `histogram` pour sa courbe |
| Déclarations | O2 `statements`, puis objets `statement` dans Postgres une fois retenues (`cases-workspace.md` K43) | Volume borné aux intervenants suivis |
| Empreintes | Option A : calculées à la volée ; option B : Postgres `phrase_fingerprints` | Découverte des formules (O2 n'a ni index de fragments ni recherche vectorielle) |
| Variantes | Postgres `pgvector` (existant pour les documents) | Similarité de sens |
| Actionnariat, liens, affiliations | Ontologie Postgres (graphe par requête récursive, existant) | Contexte des propos |

O2 garde son rôle d'archive brute et de requêtes temporelles ; la
découverte et le croisement avec l'ontologie se font dans Postgres et en
Rust.

## 7. Garde-fous

### DA7 — Vocabulaire descriptif et contradictoire

- Libellés : « formule récurrente », « reprise mot pour mot »,
  « variante proposée », « apparition synchronisée », « affiliation non
  mentionnée », « lien sourcé ». L'outil ne qualifie ni les personnes, ni
  les médias, ni leurs intentions.
- Écarter une proposition demande un motif, journalisé (comme en K45).
- Toute citation est vérifiée mot pour mot côté serveur ; une
  explication du LLM est marquée « proposée par l'assistant » et relue
  avant tout export.

### DA8 — Données personnelles

- Des propos peuvent révéler des opinions, données sensibles au sens du
  RGPD (art. 9). La base visée est l'exercice journalistique ou
  documentaire (art. 85 RGPD, art. 80 loi Informatique et Libertés),
  pour des **personnes publiques dans leur rôle public** et des
  **médias**, jamais des particuliers.
- La base de traitement est exigée par l'enquête (K38) ; la lecture est
  journalisée (`case_reads`).

### DA9 — Usage borné

L'outil est limité :
- aux **sources déclarées** de l'org ;
- aux **personnes publiques** suivies explicitement dans une enquête ;
- sans collecte automatisée de personnes (DA6) ;
- sans identification par le visage ;
- avec la voix seulement par enrôlement consenti (K19).

### DA10 — Mesurer avant de mettre en avant

Les seuils (k, w, n, m, T) et le taux de faux positifs se mesurent sur
un corpus réel avant toute mise en avant. La captation de 24 h prévue
comme critère de sortie du lot 1 de `media-ingest.md` sert de banc :
- volume d'empreintes par jour et par flux ;
- part des formules candidates jugées pertinentes par un humain ;
- temps de calcul de l'option A sur Pi 5 pour 7 jours × 4 flux.

## 8. Pack `media_watch` (proposé)

Contenu :
- le type `talking_point` et le lien `repeats` ;
- une étiquette documentée sur l'objet média (actionnariat, financement,
  statut), toujours accompagnée de sa source ;
- trois modèles de pages : « Note de veille : formule récurrente »,
  « Cartographie de l'actionnariat », « Fiche intervenant »
  (affiliations sourcées, passages à l'antenne).

Il requiert le pack Couverture médiatique (actionnariat, chaînes,
émissions) et `investigation` (déclarations).

## 9. Lots (après le lot L7 de `cases-workspace.md`)

| Lot | Contenu | Sortie vérifiable |
|---|---|---|
| **DL1** | Fonction `pnex-core` de fragments et empreintes (normalisation FR, winnowing, filtre de fréquence) avec tests ; outil assistant `find_repeated_phrases` (option A) ; propositions en file de revue | Sur un corpus de test : une formule injectée chez 4 locuteurs est trouvée avec ses 4 occurrences, aucune tournure courante proposée |
| **DL2** | Type `talking_point`, lien `repeats`, ligne « Formule » sur la frise (première occurrence, diffusion) | Une formule retenue rejoue sa diffusion sur la frise |
| **DL3** | Mesures de pluralisme : temps d'antenne par chaîne et par groupe, propositions « affiliation non mentionnée » | Tableau du temps d'antenne d'une semaine, deux affiliations proposées et sourcées |
| **DL4** | Variantes (`pgvector` + classifieur fermé) ; étiquette documentée des sources et antériorité | Une variante proposée, marquée comme telle |
| **DL5** | Option B si DA10 la justifie : empreintes au fil de l'eau, fenêtre glissante | « Où d'autre cette phrase a-t-elle été dite ? » en < 200 ms sur Pi 5 |

## 10. Questions ouvertes

1. Emplacement du composant (§5) : A d'abord, puis B ? Ou B tout de
   suite pour les flux d'une org dont c'est le métier ?
2. Étiquette documentée des sources : quelle référence fait foi
   (registre public, déclaration de l'éditeur, choix motivé de l'org) ?
3. Corpus de référence pour le filtre de fréquence : par org (ses
   propres flux) ou un corpus français général livré avec PNEX ?
4. Le périmètre sans réseaux sociaux (DA6) est-il suffisant pour les
   usages visés ?
