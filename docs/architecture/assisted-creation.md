# PRD — Création assistée : noyau et pack

> Statut : **v0.3, 2026-10-16**, redéfinition du pré-PRD v0.2 après le chantier « argent et navigation » des dossiers (`cases-money.md`). Décisions **D225–D237 validées le 2026-10-16** ; la signature C2PA est reportée (D235). Lexique arbitré (§9). Roadmap : P2.22. Aucun code.
>
> Ce qui change par rapport à la v0.2 : la v0.2 posait un modèle à part (Projet, Scène, Plan, Personnage…). La v0.3 applique la règle tirée de K55 (« un versement est un événement ») : **aucun type nouveau quand un type existant plus un rôle suffit**. Une scène devient un événement, une production devient un espace de travail comme le dossier, et les vues des dossiers (frise, Histoire, graphe, carte, Argent) servent telles quelles.

## 1. Objectif

Produire des contenus (vidéos, récits) **depuis les données de la plateforme**, avec une aide à chaque étape : idée, structure, scènes, cadrages, assets, génération, export prêt à monter.

Trois usages, un seul modèle :

| usage | point de départ | ce qu'il ajoute |
| --- | --- | --- |
| **Enquête** | un dossier existant | les scènes **impliquent directement** les objets du dossier (personnes, lieux, pièces) ; chaque cadrage garde le lien vers sa source ; les vues du dossier deviennent des cadrages |
| **Vidéo** (explication, tutoriel, chronique) | un sujet | parties (accroche, développement, conclusion), métadonnées de publication (titre, description, chapitres horodatés, miniature), sous-titres depuis la transcription |
| **Fiction** | une prémisse | personnages, fils, chapitres, temps du récit relatif (« jour 3 »), analyse de répartition |

L'usage est une propriété de la production (`profile`), pas trois packs. Il choisit les modèles de page, les sections de fiche et la vue par défaut.

## 2. Ce que les dossiers ont appris

| constat du chantier dossiers | conséquence pour la création |
| --- | --- |
| Un versement n'est pas un type : c'est un événement avec un montant et deux rôles (K55). Il apparaît tout seul sur la frise, l'Histoire, la carte. | Une **scène** est un événement. Elle apparaît sans code dans la frise et l'Histoire (fils des personnages). |
| La vue Histoire (storyline) trace les acteurs comme des fils dans le temps. | C'est exactement la vue « fils narratifs » d'un logiciel d'écriture. Rien à construire. |
| Les pièces et affirmations sont une couche repliée en pastilles (K49), la légende filtre par famille. | Les assets d'une scène (images, modèles 3D) se replient de la même façon dans le graphe. |
| Le calcul de la vue Argent est en Rust, rendu en SVG maison, sans bibliothèque. | Même règle pour la vue Plan et le tableau des cadrages : calcul au noyau, rendu maison. |
| Le bénéficiaire effectif est une **déduction** affichée comme telle (K56). | L'analyse de répartition (temps de présence par personnage) est un calcul affiché, jamais saisi. |

## 3. État de l'art (complément du 2026-10-16)

| source | idée retenue |
| --- | --- |
| Narratologie : *fabula* et *syuzhet* (formalistes russes, Bal) | **deux ordres** : l'ordre chronologique des faits (fabula) et l'ordre où on les présente (syuzhet). Un flashback = un écart entre les deux. |
| Ontologies narratives (CMN 2016) | trois dimensions : le réseau des faits, le texte qui les raconte, le lien entre les deux. |
| Graphes de récit (storygraph) | une table à un seul horodatage ne porte qu'une chronologie ; séparer la couche « faits » de la couche « présentation ». |
| *Visual Story-Writing* (2024) | les éléments de la fabula (acteurs, temps, lieu, faits) et ceux du récit (personnages, temporalité, espace, focalisation) se manipulent visuellement. |
| OpenTimelineIO | arbre Timeline → Stack → Track → Clip / Gap, référence de média externe ou manquante, métadonnées libres ; JSON imbriqué sans instanciation. Simple à écrire en Rust sans dépendance. |
| C2PA (manifeste de provenance) | standard ouvert pour signer « contenu généré » dans le fichier ; SDK Rust sous MIT/Apache-2.0, en 0.x (API instable). |

Notre traduction : **fabula = `occurred` de l'événement** (déjà au noyau, avec son incertitude), **syuzhet = position dans une partie** (lien ordonné, D229). La frise et l'Histoire montrent la fabula ; la vue Plan montre le syuzhet. Un flashback se voit en comparant les deux.

## 4. Décisions proposées

### D225 — La production est un espace de travail, comme le dossier

- Le noyau généralise l'espace de travail du dossier (`CaseWorkspace`) : **un conteneur, ses membres, ses vues**. Le lien d'appartenance `in_case` devient `in_workspace`, ouvert à tout type déclaré `workspace: true` (beta : renommage sans compatibilité).
- Le dossier reste un type système avec ses réglages d'enquête (base légale, seuils de co-présence…). La **production** est un type du pack, déclaré conteneur d'espace de travail.
- Gain : frise, Histoire, graphe, carte, Argent, chemin (K51), légende (K49) et recherche servent la production sans une ligne de vue nouvelle.
- Profil Enquête : une production **référence** un dossier (lien `draws_on`), elle ne copie rien. Ses membres sont des objets du dossier ou des objets propres.

### D226 — La scène est un événement

- Une scène est un événement du noyau (`event`) membre d'une production. Pas de type « scène ».
- Personnages, lieux, objets, pièces s'y attachent par `involves`, avec les rôles existants (`actor`, `context`, `evidence`, `source`).
- Une scène sans date est un membre non daté (il en existe déjà dans les dossiers) : la frise l'ignore, la vue Plan la montre.

### D227 — Un pack peut ajouter des rôles d'attachement

- Aujourd'hui la liste des rôles de `involves` est fermée au noyau (7 valeurs). Le noyau accepte des **rôles déclarés par un pack** (`involves_roles` dans le YAML), nommés et traduits comme le reste.
- Création : `pov` (point de vue, la focalisation), `mention` (cité sans être présent). Finance (K60) en profitera (`intermediary`).
- Les rôles du noyau gardent leur sens partout ; un rôle de pack disparaît des sélecteurs quand le pack est désinstallé, les liens existants restent lisibles.

### D228 — Temps relatif

- Un espace de travail peut porter une **origine** et un mode d'affichage : absolu (dates) ou relatif (« J+3 », « T+12 min »).
- Fiction : origine arbitraire, temps du récit en jours ou en heures depuis l'origine. Analyse de cause racine (RCA) : « T+12 min » depuis l'incident. Même frise, même curseur.
- Stockage inchangé : `occurred` reste un intervalle daté ; seul l'affichage change.

### D229 — Liens ordonnés et vue Plan

- Le noyau ajoute `ordered: true` sur un type de lien : le lien porte une position gérée par le noyau (rang fractionnaire, réordonner sans renuméroter).
- Le pack déclare `part_of` (scène → partie, partie → production) ordonné. Partie = chapitre en fiction, accroche/développement/conclusion en vidéo.
- **Vue Plan** (noyau) : l'arbre ordonné d'un conteneur, glisser-déposer, dans la coquille d'éditeur (D222). Elle sert aussi aux étapes d'une procédure (Pages L6) et d'un ordre de travail.
- Les deux ordres côte à côte : la vue Plan peut afficher la date de chaque scène ; une scène présentée avant une scène plus ancienne est marquée « retour en arrière ».

### D230 — Un personnage n'est jamais une personne réelle

- Le pack déclare `character` (fiction). Une personne réelle (`person` du pack Enquête) n'est **jamais convertie** en personnage : en profil Enquête, la scène l'implique directement.
- Une production peut mêler les deux (docu-fiction), mais la différence se voit partout : famille de couleur distincte dans la légende, pastille « réel » sur les personnes.
- Remplace le « une Personne devient Personnage par lien » de la v0.2 : le lien entretenait l'ambiguïté qu'on veut éviter.

### D231 — Assets sur tout objet

- Lien système `depicts` (média → tout objet), avec un attribut `role` : `reference` (image de référence), `model` (3D), `voice`, `render` (rendu d'un cadrage), `still` (photo). Il généralise `placed_on`, réservé aux POI.
- Fiche de tout objet : une section **Galerie** (images en vignettes, modèle 3D dans la visionneuse, audio dans le lecteur partagé D220). Les versions sont celles du média (D221).
- Graphe : les assets sont une famille repliée en pastilles par défaut, comme les preuves (K49).
- Sert aussi : photos et 3D d'un équipement, d'un POI, d'un appareil.

### D232 — Le cadrage : un type du pack, le résultat est un média

- Type du pack `shot` (cadrage) : cadrage, mouvement, durée visée, dialogue, **prompt**, paramètres, fournisseur. Lien ordonné `part_of` vers sa scène.
- Ce qu'il produit (image générée, rendu d'une vue, import) est un média `depicts` avec `role: render`. Régénérer = nouvelle version du média, les précédentes restent.
- Profil Enquête : un cadrage peut être un **rendu d'une vue** du dossier (Histoire, frise, graphe, carte, Argent) à un instant du curseur, avec le lien vers la vue et ses filtres : visuel traçable.

### D233 — Données personnelles masquées par défaut à l'export

- Le noyau ajoute `personal: true` sur une définition de type (pack Enquête : `person`). Un objet d'un tel type est **masqué ou flouté à l'export et sur les pages publiques**, sauf marqué « public » par un humain (étiquette système D218, tracée dans la provenance).
- Sécurité par défaut : c'est la publication qui demande un geste, pas la protection. Un objet marqué « mineur » ne peut jamais être rendu public.
- Sert aussi la purge RGPD des dossiers (K16, K64) : la liste des objets personnels est déjà calculable.

### D234 — Génération par les fournisseurs de l'organisation

- Inchangé sur le fond : image, vidéo, voix, 3D via les fournisseurs de l'organisation, avec sa clé (extension de D119). **Aucun calcul GPU sur la plateforme.**
- Clés au coffre (D110–D118), appels sortants par la garde (R8), aucune clé dans un DTO de lecture (R4, R16).
- Personnages cohérents : les médias `depicts` de rôle `reference` d'un personnage sont transmis au fournisseur quand il le permet.

### D235 — « Contenu généré » se voit et se transporte

- Chaque média généré porte au noyau `generated: true`, le fournisseur et le prompt (provenance D184).
- Pastille « généré » partout où le média s'affiche ; rappel à l'export.
- **Reporté** : manifeste C2PA signé dans le fichier exporté. On le reprendra quand une plateforme de diffusion l'exigera ou quand le SDK Rust (MIT/Apache, encore en 0.x) sera stable ; la donnée `generated` du noyau suffit à l'écrire plus tard sans migration.

### D236 — Exports

- **Timeline** : OpenTimelineIO (JSON) et FCPXML, écrits en Rust sans dépendance. Partie = piste ou marqueur, cadrage = clip, cadrage sans média = `MissingReference` (le monteur voit le trou).
- **Vidéo des vues** (Histoire, frise, graphe, carte, Argent) : rendu dans le navigateur (WebCodecs), 1080×1920 et 4K. Repli à définir là où WebCodecs manque (Q2). Le serveur ne rend rien.
- glTF du graphe et des lieux pour un rendu externe : plus tard (C4).

### D237 — L'assistant écrit un brouillon, jamais un fait

- Outils de création : proposer des scènes et des cadrages depuis une production (et, en profil Enquête, depuis le dossier). Écriture par les services partagés avec l'UI (§9.3 de `ai-assistant.md`), `expected_version`, `can_write`.
- **Profil Enquête** : chaque scène proposée **implique** des objets du dossier ; une phrase sans objet source est marquée « non sourcée » (même traitement que les versements sans pièce, K57). L'assistant n'invente aucun fait sur une personne réelle.
- Fiction : liberté totale, sans jamais toucher un objet `personal`.

## 5. Noyau et pack : le partage

| | noyau | pack `creation` |
| --- | --- | --- |
| Modèle | espace de travail générique (D225), rôles de pack (D227), temps relatif (D228), liens ordonnés (D229), `depicts` (D231), `personal` (D233), `generated` (D235) | types `production`, `part`, `character`, `shot` ; liens `part_of` (ordonné), `draws_on` ; rôles `pov`, `mention` ; modèles de page par profil |
| Vues | frise, Histoire, graphe, carte, Argent (existantes) ; **vue Plan** ; galerie de fiche ; visionneuse 3D | aucune : un pack est du YAML |
| Calcul | analyse de répartition (présence par personnage, lieu, fil, partie) en Rust | — |
| Export | OTIO, FCPXML, vidéo des vues | — |
| Assistant | outils (D237), fiche `assistant-kb` | règles d'aide du profil dans la fiche |

Le **tableau des cadrages** (vignettes ordonnées, statut de génération) est une vue Plan en grille : même donnée, même composant, deux mises en page.

Règle inchangée : une fonctionnalité qui n'a de sens que pour la création va dans le pack ; tout ce que la liste « sert aussi » ci-dessus justifie va au noyau.

## 6. Ce qu'on réutilise

| besoin | brique existante |
| --- | --- |
| scènes dans le temps, fils des personnages | événements, frise, vue Histoire |
| liens entre personnages | ontologie (liens temporels), graphe, chemin K51 |
| lieux réels | `place` (pack Enquête), POI, carte |
| lecture | lecteur partagé (D220) |
| révisions | historique de versions (D221) |
| édition | coquille d'éditeur (D222) |
| médias | médias versionnés ; type `model/gltf-binary` déjà reconnu |
| texte assisté | assistant, LLM de l'organisation (D119) |

## 7. Règles d'étude (salle blanche)

PNeX est sous MIT. Une partie du corpus est sous copyleft (GPL/AGPL). Règles de l'annexe B de `ontology.md` :

| règle | détail |
| --- | --- |
| Inspiration, pas reprise | On étudie des **concepts et des comportements**, jamais du code, des textes d'interface, des questionnaires ou des icônes. |
| Aucun extrait de code dans les notes | Ni copier-coller, ni pseudo-code calqué sur une source copyleft. |
| Schéma au niveau concept | Entités, relations, cardinalités ; jamais de noms de tables ou de colonnes. |
| Nos propres mots | Les sections de fiche (personnage, lieu…) sont rédigées par nous. Aucune liste de questions recopiée. |
| Captures internes seulement | Elles ne sont pas versionnées. |
| Séparation | Les notes vont dans `research/assisted-creation/` ; le code s'écrit depuis le PRD. |
| Outils externes ≠ briques | Un outil copyleft ou en Python peut servir **à côté** (montage, rendu), jamais **dans** PNeX. |

Corpus (licences à vérifier sur le `LICENSE` réel) :

| projet | licence | ce qu'on y regarde |
| --- | --- | --- |
| bibisco | GPL | prémisse, fabula, fils, chapitres → scènes, fiches, répartition |
| novelWriter | GPL | arbre, statuts, métadonnées par scène |
| Manuskript | GPL | méthode « flocon » (du résumé d'une phrase au plan détaillé) |
| Plottr, Scrivener | propriétaires | grille fils × temps, tableau de liège, compilation |
| Storyboarder | à vérifier | cadrage, durée, dialogue, animatique |
| Kitsu | AGPL | suivi de production : séquences, plans, assets, statuts |
| OpenTimelineIO | Apache-2.0 | format d'échange, écrit en Rust |
| FCPXML | spécification publique | export vers les monteurs |
| OpenMontage | AGPLv3, Python | outil externe uniquement |

## 8. Visionneuse 3D (noyau, premier lot)

- Affiche un GLB/glTF ; montre le squelette ; oriente les articulations ; **enregistre une pose nommée** comme donnée de l'objet ; lit les animations du fichier ; rend une **vignette** PNG dans le navigateur.
- Candidate : three.js (MIT), en JS embarqué comme `js/graph.js`. Choix final avec la règle des dépendances (maintenance, licence).
- Hors lot : rigging automatique, animation pilotée, génération 3D.

## 9. Lexique (arbitré le 2026-10-16)

Trois mots de la v0.2 étaient déjà pris dans l'UI ; ils sont remplacés :

| v0.2 | déjà utilisé pour | retenu |
| --- | --- | --- |
| Projet | projet firmware, projet du Vision Lab | **Production** / Production |
| Scène | scène 360° du Studio | **Séquence** / Sequence (l'objet reste un événement du noyau) |
| Plan | plan d'étage | **Cadrage** / Shot |

Les lignes Production, Partie, Séquence, Personnage, Cadrage, Galerie et Généré sont au §7 de `design-system.md`. Les règles de `lexicon_guard.rs` arrivent avec le premier lot qui ajoute ces mots aux `.ftl`.

Dans le reste de ce document, « scène » désigne la notion narrative ; l'interface dit « Séquence ».

## 10. Lots

| lot | contenu | dépend de |
| --- | --- | --- |
| C0 | étude salle blanche, notes (lexique : fait, §9) | — |
| C1 | **noyau** : `depicts` + galerie de fiche + visionneuse 3D (D231, §8) | — |
| C2 | **noyau** : espace de travail générique (D225), rôles de pack (D227), liens ordonnés + vue Plan (D229), temps relatif (D228) | C0 |
| C3 | **pack** `creation` : production, partie, personnage, profil Fiction ; analyse de répartition | C2 |
| C4 | profil Enquête (`draws_on`, `personal` + masquage D233, rendus de vues comme cadrages) et profil Vidéo (métadonnées de publication, sous-titres) | C3 |
| C5 | cadrage + génération d'image par fournisseur d'org + tableau des cadrages ; `generated` (D232, D234, D235) | C3 |
| C6 | exports : OTIO, FCPXML, vidéo des vues (D236) | C5 |
| C7 | assistant (D237) : outils + fiche | C3 (au plus tard avec chaque lot visible) |

C1 et C2 sont indépendants et utiles hors création : on peut les livrer sans le pack.

## 11. Questions ouvertes

1. Fournisseurs de génération : liste blanche initiale, format commun des paramètres.
2. WebCodecs : disponible sur web, desktop WebKitGTK, Android ? Repli (export image par image, assemblage côté navigateur) ?
3. Sections de la fiche personnage : lesquelles, en nos mots.
4. `in_workspace` : un objet membre de deux espaces de travail (un dossier et sa production) — autorisé (proposé) ; la suppression d'un membre ne retire que l'appartenance.
5. ~~Lexique~~ : tranché, « Séquence » (§9).
6. C2PA, quand on le reprendra : signer avec quelle clé (PKI de l'org, D153 ?) et seulement à l'export ?

## 12. Critères d'acceptation (premiers lots)

- C1 : une photo et un GLB s'attachent à un équipement et à un personnage ; la galerie les montre ; une pose nommée s'enregistre et produit une vignette.
- C2 : un dossier existant s'ouvre sans régression (e2e `cases.spec`) après le passage à `in_workspace` ; une production fictive montre ses scènes sur la frise en « J+n » et dans la vue Plan dans l'ordre de présentation ; un retour en arrière est marqué.
- C3 : la vue Histoire d'une production fictive trace les fils des personnages sans code de vue ajouté.

## 13. Hors périmètre

- Montage dans PNeX : on exporte vers un monteur, on ne remplace pas le monteur.
- Calcul GPU, modèles hébergés par la plateforme.
- Animation automatique de personnages, rigging automatique.

## 14. Avancement

### 2026-10-16 — C1 livré (non commité)

- **Noyau** : lien système `depicts` (média → tout objet), attributs `role` (`reference`, `model`, `voice`, `pose`, `render`), `name`, `pose` (quaternions par articulation, JSON validé côté serveur), `model` (le modèle 3D d'une pose).
- **Médias** : format GLB (glTF binaire 2.0) admis par la liste blanche, type de média `mesh` (« Modèle 3D »), servi en octets opaques (SEC-5).
- **Fiche d'objet** : onglet **Galerie** sur tout objet sauf un média ; ajout depuis la bibliothèque ou téléversement ; rôle déduit du type de média ; retirer = fermer le lien.
- **Visionneuse 3D** (three.js 0.186.1, MIT, dans `viewers.js`, désormais minifié) : orbite, animations du fichier, squelette, articulation + 3 curseurs, pose de repos, **pose nommée** = angles + vignette PNG enregistrée comme média de la galerie, ré-application d'un clic. Aussi sur la fiche d'un média GLB.
- **Assistant** : fiches `ontology.md` (galerie, poses, modèle sans poids) et `media.md`.
- **Lexique** : Modèle 3D, Pose, Articulation ; garde : « assets », « bone ».
- **Tests** : noyau (`pose_valid`), tri des formats (GLB v2 seulement), backend `gallery_depicts_a_model_and_its_poses` (rôles, pose invalide, lecteur → 403), e2e `gallery.spec.ts` (fr/en) avec un GLB articulé généré en mémoire (`e2e/src/rig_glb.ts`, aucun binaire versionné).
- **Constat** : un GLB issu d'un générateur automatique peut avoir un squelette sans poids (tout sur la hanche) : les articulations tournent, le maillage ne bouge pas. La fiche de l'assistant donne le geste (corriger dans l'outil de modélisation).

Reste pour C1 : choisir une articulation en cliquant sur le modèle (aujourd'hui : liste), vignettes servies en petite taille (aujourd'hui : l'image entière).

## 15. Sources

- [Fabula and syuzhet](https://en.wikipedia.org/wiki/Fabula_and_syuzhet)
- [An ontology of narratives (CMN 2016)](https://drops.dagstuhl.de/storage/01oasics/oasics-vol053_cmn2016/OASIcs.CMN.2016.4/OASIcs.CMN.2016.4.pdf)
- [Don't Blink (storygraph)](https://storygraph.substack.com/p/dont-blink)
- [Visual Story-Writing (2024)](https://arxiv.org/pdf/2410.07486)
- [OpenTimelineIO, architecture](https://opentimelineio.readthedocs.io/en/v0.13/tutorials/architecture.html)
- [C2PA Rust SDK](https://docs.rs/crate/c2pa/0.15.0)
