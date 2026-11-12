# PRD — Pièces et documentation : la bonne pièce et sa doc en quelques secondes

> **Statut** : proposition (2026-11-12), **rien d'implémenté**, ne rien coder
> avant validation. Décisions D960–D971 **proposées**, questions Q-PD1–Q-PD9
> ouvertes. Roadmap : P2.39.
> **Dépend de** : ontologie 0.2.0 (livrée), GMAO générique (`ontology.md`
> annexe C : `spare_part`, `work_order`, stock, fournisseurs), Pages
> (procédures L6), médias et annotations (D147, D219), recherche documentaire
> (`doc-search.md`), vision (`ml-vision.md`).
> **Périmètre** : tout corps de métier de la maintenance (froid, CVC,
> cuisines professionnelles, électricité, ascenseurs, machines de
> production…). Aucun type ni aucune règle propre à un métier dans le moteur :
> tout vit dans des packs (garde `tests/engine_genericity.rs`).

---

## 1. Problème

Retrouver une pièce de rechange est une perte de temps quotidienne en
maintenance :

- des dizaines de milliers de références chez un seul distributeur, des
  milliers de fabricants ;
- une documentation éparpillée : vues éclatées en PDF, nomenclatures,
  schémas électriques, frigorifiques ou hydrauliques, notices de montage,
  parfois des modèles 3D, chacun chez un fabricant différent ;
- des références qui changent (remplacée, équivalente, différente selon la
  plage de numéros de série) : la première cause d'erreurs et de retours ;
- un appareil mal identifié chez le client : plaque illisible, modèle
  inconnu, historique perdu.

| Persona | Contexte | Douleur |
|---|---|---|
| **Chiffreur** (bureau) | Reçoit le diagnostic du technicien, doit sortir un devis | Trouver la bonne référence, vérifier qu'elle convient au numéro de série, gérer les retours, redemander un prix périmé |
| **Technicien** (terrain, mains sales, réseau médiocre) | Démonte et remonte un appareil | Identifier la pièce, savoir comment elle se monte (ordre, couples, raccordements), avoir le schéma sous les yeux sans appeler le bureau |

**Principe directeur** : un appareil référencé et tagué par QR code donne
sa pièce et sa doc en **moins de 10 secondes** ; un appareil inconnu reste
trouvable par photo de plaque, photo de pièce ou recherche texte en moins
d'une minute.

## 2. Objectifs et non-objectifs

### Objectifs
1. **Fiche pièce riche** : vue éclatée avec repère, modèle 3D, schémas,
   notice, procédure de montage, photos terrain, compatibilités.
2. **Recherche multimodale** : QR, référence (partielle, ancienne), texte
   libre, numéro de série, photo de plaque, photo de pièce, navigation dans
   la vue éclatée.
3. **Raccourci QR** : scanner un appareil ouvre son modèle, son numéro de
   série, sa nomenclature filtrée, sa doc et son historique.
4. **Boucle d'apprentissage** : chaque correction (mauvaise référence,
   retour, équivalence trouvée) enrichit la base.
5. **Terrain d'abord** : mobile, consultable hors ligne pour les appareils
   synchronisés.

### Non-objectifs (cette version)
- Pas de recherche concurrente dans le catalogue propre d'un distributeur.
- Pas d'aspiration de sites tiers ; les prix sont un module à part (§9),
  à la demande seulement.
- Pas de devis ni de facturation : on produit un **panier de pièces**
  (l'OT de l'annexe C, état `requested`, liens `uses_part`), exportable.
- La GMAO reste celle de l'annexe C ; ce PRD n'y ajoute que l'historique
  utile au dossier de l'appareil.

## 3. Concepts, portés sur l'ontologie (D960)

**D960 (proposée)** : tout est objet ou lien d'ontologie dans un pack
`parts_docs` (`requires: [maintenance]`), sans table métier dédiée ; seuls
le jeton QR (§5.4) et l'index de recherche des références (§5.1) sont du
noyau, parce qu'ils servent tous les types.

| Concept | Porté par | Remarque |
|---|---|---|
| Fabricant | `organization` du graphe, lien `manufactures` → `equipment_model` | Une fiche par société, comme les fournisseurs (annexe C1) |
| Modèle d'appareil | type `equipment_model` (marque, modèle, variantes, famille) | Porte la doc et la nomenclature |
| Appareil installé | **tout objet** d'équipement existant (machine, pompe, compresseur… ou type de l'org), lien `instance_of` → `equipment_model`, propriétés numéro de série et date de fabrication | Pas de nouveau type « instance » : l'équipement est déjà là, avec ses capteurs (D181) |
| Pièce | `spare_part` (annexe C) | Étendu par le pack : famille, caractéristiques, photos |
| Référence de pièce | type `part_reference` (référence brute, normalisée, nature : origine, distributeur, ancienne ; source), lien `refers_to` → `spare_part` | N références par pièce |
| Relation entre pièces | liens `supersedes`, `equivalent_to`, `compatible_with` (attribut conditions) | Validés par une personne (proposals) |
| Ligne de nomenclature | lien `bom_line` `equipment_model` → `spare_part`, attributs repère, quantité, série de/à, date de/à | Le filtre par numéro de série lit ces bornes |
| Document technique | média de la médiathèque (PDF, image, glTF) lié au modèle (`documents`), nature : vue éclatée, nomenclature, schéma, notice, manuel, fiche, 3D | Stockage RustFS comme tout média |
| Ancre dans un document | **calque d'annotations** sur le média (D147, D219) : page, zone, hotspot de repère, nœud 3D | Un seul mécanisme d'ancrage dans la plateforme |
| Procédure de montage | **procédure Pages L6** (`pages.md` P9), modèle par `equipment_model` × pièce | Étapes, outils, couples, sécurité, temps standard |
| Note terrain | page ou commentaire rattaché à la pièce, au modèle ou à l'appareil, **proposition** validée par un référent | Photos via la médiathèque |
| Correction de pièce | type `part_correction` (modèle, plage de séries, référence fausse, bonne, source, statut) | Alimente recherche et alertes |
| Panier d'intervention | OT de l'annexe C (`uses_part`) | Remonte au chiffreur |

## 4. Parcours

### 4.1 Technicien, appareil tagué (< 10 s)
1. Il scanne le QR de l'appareil.
2. La fiche s'ouvre : modèle, numéro de série, site, dernières
   interventions, pièces déjà changées, mesures si un capteur est posé.
3. Onglet **Pièces** : la vue éclatée, **filtrée sur sa plage de séries** ;
   il touche le repère et obtient la fiche pièce.
4. Fiche pièce : référence valide aujourd'hui (remplacements résolus), 3D,
   procédure de montage, schéma surligné, notes terrain.
5. Il ajoute la pièce au panier (l'OT), qui remonte au chiffreur.

### 4.2 Technicien, appareil inconnu
1. Photo de la plaque : OCR assisté par l'assistant (marque, modèle, série,
   date de fabrication).
2. PNEX propose les modèles candidats avec un score ; il confirme.
3. Il crée l'appareil et l'**associe à une étiquette QR vierge** scannée
   sur place ; la fois suivante, c'est le cas 4.1.
4. En dernier recours, photo de la pièce : la similarité visuelle propose
   des pièces candidates parmi les modèles de la marque.

### 4.3 Chiffreur
1. Il part du panier, d'un appareil, d'une référence ou d'une recherche
   texte (« résistance rinçage 6 kW »).
2. Par ligne : référence d'origine, remplacements, équivalents validés,
   corrections passées (« 3 retours sur cette référence pour ce modèle »).
3. Alerte si la référence sort de la plage de séries de l'appareil.
4. Il valide la liste et l'exporte ; les prix viennent de §9 quand il existe.
5. Un retour déclaré crée une `part_correction` et corrige la nomenclature.

## 5. Exigences

### 5.1 Recherche (D961)
- **Un seul champ** : référence exacte, partielle, sans séparateurs ou
  ancienne ; texte libre français ou anglais ; numéro de série.
- **D961 (proposée)** : recherche hybride lexicale + vectorielle dans
  PostgreSQL (pgvector), sur l'index de `doc-search.md` ; les références
  normalisées (casse, séparateurs, préfixes fabricant) ont leur propre index
  exact et préfixe.
- **Résolution des remplacements** : une référence remplacée renvoie
  l'actuelle avec son historique (chaîne `supersedes`).
- Filtres : fabricant, modèle, famille, caractéristiques.
- Entrées image : photo de plaque (OCR), photo de pièce (similarité).
- Cibles : **< 300 ms** texte ou référence sur 100 000 pièces, < 3 s image.

### 5.2 Documentation technique
- Viewer PDF qui **s'ouvre à la page et à la zone** de l'ancre, sans
  téléchargement.
- **Vue éclatée interactive** : hotspots du calque d'annotations ; toucher
  un repère ouvre la pièce, la fiche pièce surligne son repère.
- **Viewer 3D** glTF/GLB (STEP converti à l'ingestion) : nœud ↔ pièce, vue
  isolée, éclatement si les données le permettent. Bibliothèque à licence
  permissive obligatoire.
- **Schémas** : surlignage du composant référencé (repère, borne).
- Studio 360 : photo d'un local technique avec hotspots vers les appareils.

### 5.3 Aide au montage
- Procédure L6 pas à pas : texte, photo, repère 3D par étape, outils,
  couples, temps standard.
- **Bloc sécurité** en tête, fourni par le pack métier (consignation
  électrique, récupération de fluide, habilitations…).
- Notes terrain proposées depuis le terrain, validées par un référent.
- Mode « mains sales » : gros boutons, navigation par étapes, lecture à
  voix haute optionnelle.

### 5.4 QR code et parc (D962–D964)
- **D962 (proposée)** : un QR porte une **URL courte vers un jeton stable**
  (table noyau `qr_tags` : jeton, org, objet nullable, statut vierge /
  associé / révoqué), jamais de donnée en clair. Le jeton sert à tout objet
  (équipement, pièce, emplacement de stock de l'annexe C), pas seulement
  aux appareils.
- **D963 (proposée)** : planches d'étiquettes **vierges** pré-imprimées,
  associées sur place par scan ; planche d'étiquettes **associées** pour une
  sélection (lot C-QR1 de l'annexe C).
- **D964 (proposée)** : un scan sans droit affiche une page minimale
  configurable par org (marque, modèle, contact du mainteneur), **sans
  donnée client** ; servie hors de l'origine de l'app (R11–R13) ; jeton
  non énumérable, révocable.
- Fiche appareil : identité, site, interventions, pièces changées (date,
  technicien), documents, mesures.

### 5.5 Ingestion du contenu (D965–D966)
Cœur du coût et de la valeur.
- Import des PDF fabricants (vues éclatées, nomenclatures, manuels) dans
  la médiathèque.
- **D965 (proposée)** : extraction par **flows et pipelines PNEX**
  (`pipeline-engine.md`), jamais un traitement à côté :
  1. découpage et classification des pages (vue éclatée, nomenclature,
     schéma, texte) ;
  2. extraction des tableaux de nomenclature (repère, référence,
     désignation, quantité, plage de séries) ;
  3. détection des repères numérotés sur les vues éclatées → hotspots
     semi-automatiques ;
  4. liaison repère ↔ ligne de nomenclature ↔ `spare_part` ;
  5. **validation humaine** côte à côte (PDF à gauche, données à droite) :
     tout ce qui est extrait entre en **proposition**, jamais en fait.
- **D966 (proposée)** : chaque donnée garde sa provenance (document, page,
  date d'import, validateur) — provenance standard de l'ontologie.
- Import CSV/Excel de nomenclatures et de tarifs (imports D301).
- Import 3D (STEP, glTF) avec mapping nœuds → pièces.

### 5.6 Boucle d'apprentissage (D967)
- Un retour ou une erreur de référence crée une `part_correction`.
- Les corrections validées remontent dans la recherche et les alertes.
- **D967 (proposée)** : base **privée par org** par défaut. Une base
  commune entre orgs franchit la frontière de l'org (modèle de menace de
  `security.md`) : elle ne recevrait que des faits (référence,
  correspondance, remplacement, correction validée), jamais un document,
  une photo ou une donnée client, après modération. À trancher (Q-PD2).
- Indicateurs par pièce et par modèle : retours, corrections, confiance de
  la nomenclature.

### 5.7 Hors ligne (D968)
- **D968 (proposée)** : synchronisation des appareils d'un site ou d'une
  tournée dans l'app mobile (fiches, nomenclatures, vues éclatées,
  procédures, PDF compressés) ; 3D optionnelle (taille).
- Actions hors ligne (photos, notes, panier) en file, synchronisées au
  retour du réseau, en concurrence optimiste (version lue).

## 6. Modèle de données

Pas de tables métier (D960). Ce qui s'ajoute :

| Où | Quoi |
|---|---|
| Pack `parts_docs` | types `equipment_model`, `part_reference`, `part_correction` ; liens `manufactures`, `instance_of`, `refers_to`, `supersedes`, `equivalent_to`, `compatible_with`, `bom_line`, `documents` ; extension de `spare_part` (famille, caractéristiques) et des types d'équipement (série, date de fabrication) |
| Noyau | table `qr_tags` (D962) ; index des références normalisées (D961) ; embeddings dans l'index de `doc-search.md` |
| Médiathèque | PDF, images, glTF (RustFS) ; ancres = calques d'annotations |
| Pages | modèles de procédure de montage par modèle × pièce |

Contrat d'API, IAM, MCP et assistant suivent les règles permanentes
(`api-contract.md`, `iam.md`, `mcp.md`, `ai-assistant.md` §9) au moment de
chaque lot.

## 7. Briques réutilisées

| Brique | Usage |
|---|---|
| Ontologie 0.2.0, packs | Tous les objets du §3 |
| GMAO générique (`ontology.md` annexe C) | Pièces, OT = panier, stock, fournisseurs, QR |
| Médiathèque, `doc-search.md` | Documents, recherche texte et vectorielle |
| Annotations (D147, D219) | Ancres, hotspots de vue éclatée |
| Pages L6 | Procédures de montage |
| Vision (`ml-vision.md`) | Repères sur vues éclatées, similarité de pièce |
| Assistant | OCR assisté de plaque, extraction de nomenclatures, questions en langage naturel |
| Moteur de pipelines, flows | Ingestion et validation |
| Propositions | Validation humaine de tout ce qui est extrait ou proposé |
| Agents navigateur | Module prix §9 |
| Studio 360 | Localisation des appareils |
| Devices et capteurs | Mesures sur la fiche ; plus tard : panne → pièce probable |

## 8. Phasage

| Lot | Contenu | Critère de sortie |
|---|---|---|
| **PD0 — Socle** | Pack `parts_docs`, import CSV de nomenclatures, recherche par référence normalisée avec remplacements, fiche pièce simple | Une référence ancienne renvoie l'actuelle en < 300 ms sur 100 000 pièces |
| **PD1 — QR et parc** | Jetons QR, planches vierges, association sur place, fiche appareil, nomenclature filtrée par série, panier = OT | Du scan à la fiche pièce en < 10 s sur mobile |
| **PD2 — Doc riche** | Viewer PDF ancré, vue éclatée à hotspots (saisie manuelle), procédures de montage, notes terrain | Le technicien trouve la procédure sans quitter PNEX |
| **PD3 — Ingestion assistée** | Extraction nomenclatures et repères depuis les PDF, écran de relecture | Vue éclatée de 50 repères intégrée en < 10 min de relecture |
| **PD4 — Multimodal** | OCR de plaque, recherche par photo de pièce, 3D glTF | Plaque reconnue dans > 80 % des cas sur un jeu de test |
| **PD5 — Apprentissage et hors ligne** | Corrections, base commune modérée (si Q-PD2 dit oui), hors ligne mobile | Une correction validée apparaît chez toutes les orgs concernées |

PD1 recoupe les lots C-QR1 et C-QR2 de l'annexe C : ils se font une fois.

## 9. Extension liée (PRD séparé) : prix et fournisseurs (D969)
**D969 (proposée)** : module à part, branché sur le panier :
- récupération **à la demande** par un agent navigateur, avec le compte du
  client dans le coffre de secrets ;
- cache court par org (prix, disponibilité), cache long des
  correspondances référence ↔ modèle enrichi par les corrections ;
- comparaison entre fournisseurs (lien `supplies` de l'annexe C), marges,
  relance quand un prix est trop ancien ;
- aucune aspiration de catalogue en tâche de fond, rythme limité, respect
  des conditions d'utilisation (garde de sortie R8).

## 10. Juridique et données (D970–D971)
- **D970 (proposée)** : un document importé par une org reste **privé à
  cette org** ; seuls des faits (référence, correspondance, remplacement)
  et des corrections validées peuvent alimenter une base commune (D967).
- Modèles 3D et PDF jamais republiés ; une vue éclatée protégée se cite
  par lien vers sa source.
- **D971 (proposée)** : une page QR publique ne contient aucune donnée
  client ; une photo terrain (personnes, locaux) est floutée ou validée
  avant tout partage hors de l'org.

## 11. Critères de succès
- Temps de recherche d'une pièce par le chiffreur divisé par 5 (mesuré
  avant et après chez un pilote).
- Retours pour mauvaise référence divisés par 2.
- Plus de 70 % des interventions démarrées par un scan QR après 6 mois
  chez un client équipé.
- Le technicien n'appelle plus le bureau pour identifier une pièce sur un
  appareil tagué.

## 12. Questions ouvertes
1. **Q-PD1** Sources de nomenclatures : quels fabricants fournissent des
   fichiers exploitables (CSV, portail) plutôt que des PDF ?
2. **Q-PD2** Base commune entre orgs : oui ou non ? Si oui, qui modère
   (communauté, éditeur) et comment la frontière de l'org est-elle tenue ?
3. **Q-PD3** Étiquettes : tenue à la chaleur et à la graisse, à
   l'humidité et au froid ; NFC en option ?
4. **Q-PD4** Variantes : un même modèle commercial cache plusieurs
   nomenclatures selon le marché ou l'année ; la plage de séries
   suffit-elle ?
5. **Q-PD5** Export du panier vers un devis : format (CSV, PDF, API) et
   phase ; rappel : pas de connecteur bidirectionnel vers une GMAO tierce.
6. **Q-PD6** Pilote : quelle entreprise de maintenance valide ?
7. **Q-PD7** Viewer 3D : bibliothèque (licence permissive), conversion
   STEP → glTF côté serveur (outil, licence) ?
8. **Q-PD8** OCR de plaque : modèle local (vision) ou LLM de l'org
   (pas de LLM plateforme, D119) ?
9. **Q-PD9** Le pack `parts_docs` est-il fusionné dans `maintenance` ou
   reste-t-il un module qui s'active à part ?
