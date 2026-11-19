# PRD — Pièces et documentation : la bonne pièce et sa doc en quelques secondes

> **Statut** : **validé le 2026-11-17**, rien d'implémenté. Décisions
> D960–D971 **acceptées** ; questions Q-PD1–Q-PD9 tranchées (§12). Roadmap :
> P2.39.
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
  chacun chez un fabricant différent ;
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
1. **Fiche pièce riche** : vue éclatée avec repère, schémas,
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

**D960 (acceptée 2026-11-17)** : tout est objet ou lien d'ontologie dans le pack
`maintenance` (nouvelle version mineure, pas de pack séparé : Q-PD9), sans
table métier dédiée ; seuls
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
| Ligne de nomenclature | lien `bom_line` `equipment_model` → `spare_part`, attributs repère, quantité, série de/à, date de/à, variante (marché, tension, option ; facultative) | Le filtre de l'appareil combine série, date de fabrication et variante (Q-PD4) |
| Document technique | média de la médiathèque (PDF, image) lié au modèle (`documents`), nature : vue éclatée, nomenclature, schéma, notice, manuel, fiche | Stockage RustFS comme tout média |
| Ancre dans un document | **calque d'annotations** sur le média (D147, D219) : page, zone, hotspot de repère | Un seul mécanisme d'ancrage dans la plateforme |
| Procédure de montage | page Pages liée au `equipment_model` × pièce (L1–L2, livrées) ; **procédure Pages L6** (`pages.md` P9) quand L6 existe | Étapes, outils, couples, sécurité, temps standard |
| Note terrain | page ou commentaire rattaché à la pièce, au modèle ou à l'appareil, **proposition** validée par un détenteur de `maintenance:validate` | Photos via la médiathèque |
| Correction de pièce | type `part_correction` (modèle, plage de séries, référence fausse, bonne, source, statut) | Alimente recherche et alertes |
| Panier d'intervention | OT de l'annexe C (`uses_part`) | Remonte au chiffreur |

## 4. Parcours

### 4.1 Technicien, appareil tagué (< 10 s)
1. Il scanne le QR de l'appareil.
2. La fiche s'ouvre : modèle, numéro de série, site, dernières
   interventions, pièces déjà changées, mesures si un capteur est posé.
3. Onglet **Pièces** : la vue éclatée, **filtrée sur sa plage de séries** ;
   il touche le repère et obtient la fiche pièce.
4. Fiche pièce : référence valide aujourd'hui (remplacements résolus),
   procédure de montage, schéma surligné, notes terrain.
5. Il ajoute la pièce au panier (l'OT), qui remonte au chiffreur.

### 4.2 Technicien, appareil inconnu
1. Photo de la plaque, prise même sans réseau : elle part dans la file de
   synchronisation (D968). L'OCR tourne **côté serveur** au retour du réseau
   (modèle local du serveur, affiné par le LLM de l'org s'il est configuré,
   D119) ; aucun modèle embarqué sur le mobile (Q-PD8). Sans réseau, il
   saisit marque et modèle à la main s'il doit avancer.
2. PNEX propose les modèles candidats avec un score (en proposition sur
   l'appareil quand l'OCR arrive après coup) ; il confirme.
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
4. Il valide la liste et l'exporte en CSV (référence, désignation, quantité,
   appareil, série ; Q-PD5) ; les prix viennent de §9 quand il existe.
5. Un retour déclaré crée une `part_correction` et corrige la nomenclature.

## 5. Exigences

### 5.1 Recherche (D961)
- **Un seul champ** : référence exacte, partielle, sans séparateurs ou
  ancienne ; texte libre français ou anglais ; numéro de série.
- **D961 (acceptée 2026-11-17)** : recherche hybride lexicale + vectorielle dans
  PostgreSQL (pgvector), sur l'index de `doc-search.md` ; les références
  normalisées (casse, séparateurs, préfixes fabricant ; les règles de
  préfixe vivent en YAML dans le pack, jamais en dur dans le moteur) ont leur propre index
  exact et préfixe.
- **Résolution des remplacements** : une référence remplacée renvoie
  l'actuelle avec son historique (chaîne `supersedes`).
- Filtres : fabricant, modèle, famille, caractéristiques.
- Entrées image : photo de plaque (OCR serveur, différé si hors ligne),
  photo de pièce (similarité ; modèle d'embeddings d'image choisi en PD4
  sur les photos du pilote : ONNX, exécuté côté serveur, licence permissive).
- Cibles : **< 300 ms** texte ou référence sur 100 000 pièces, < 3 s image.

### 5.2 Documentation technique
- Viewer PDF qui **s'ouvre à la page et à la zone** de l'ancre, sans
  téléchargement.
- **Vue éclatée interactive** : hotspots du calque d'annotations ; toucher
  un repère ouvre la pièce, la fiche pièce surligne son repère.
- **Schémas** : surlignage du composant référencé (repère, borne).
- Studio 360 : photo d'un local technique avec hotspots vers les appareils.

### 5.3 Aide au montage
- PD2 : la procédure est une **page Pages ordinaire** (L1–L2, livrées) liée
  au modèle × pièce, avec la notice PDF ancrée ; P2.39 n'attend pas L6.
- Avec Pages L6 : procédure pas à pas (texte, photo, repère sur la vue
  éclatée par étape, outils, couples, temps standard, mesures, signature).
- **Bloc sécurité** en tête, fourni par le pack métier (consignation
  électrique, récupération de fluide, habilitations…).
- Notes terrain proposées depuis le terrain, validées par un détenteur de
  `maintenance:validate` (§5.8).
- Mode « mains sales » : gros boutons, navigation par étapes. Pas de
  lecture à voix haute tant que le pilote ne la demande pas.

### 5.4 QR code et parc (D962–D964)
- **D962 (acceptée 2026-11-17)** : un QR porte une **URL courte vers un jeton stable**
  (table noyau `qr_tags` : jeton, org, objet nullable, statut vierge /
  associé / révoqué), jamais de donnée en clair. Le jeton sert à tout objet
  (équipement, pièce, emplacement de stock de l'annexe C), pas seulement
  aux appareils.
- **D963 (acceptée 2026-11-17)** : planches d'étiquettes **vierges** pré-imprimées,
  associées sur place par scan ; planche d'étiquettes **associées** pour une
  sélection (lot C-QR1 de l'annexe C).
- **D964 (acceptée 2026-11-17)** : un scan sans droit affiche une page minimale
  configurable par org (marque, modèle, contact du mainteneur), **sans
  donnée client** ; servie hors de l'origine de l'app (R11–R13) ; jeton
  non énumérable, révocable. **Phasage** : en PD1, un scan sans session
  ouvre le login et un scan sans droit renvoie un 403 propre ; la page
  publique (et son hôte séparé) vient dans un lot ultérieur.
- Étiquettes : **QR seul**, imprimé sur planche au format standard ; la
  matière (polyester, tenue à la graisse, au froid, à l'humidité) est un
  choix d'achat du client, décrit dans la fiche d'aide. Pas de NFC (Q-PD3).
- Fiche appareil : identité, site, interventions, pièces changées (date,
  technicien), documents, mesures.

### 5.5 Ingestion du contenu (D965–D966)
Cœur du coût et de la valeur.
- Import des PDF fabricants (vues éclatées, nomenclatures, manuels) dans
  la médiathèque.
- **D965 (acceptée 2026-11-17)** : extraction par **flows et pipelines PNEX**
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
- **D966 (acceptée 2026-11-17)** : chaque donnée garde sa provenance (document, page,
  date d'import, validateur) — provenance standard de l'ontologie.
- Import CSV/Excel de nomenclatures et de tarifs (imports D301).

### 5.6 Boucle d'apprentissage (D967)
- Un retour ou une erreur de référence crée une `part_correction`.
- Les corrections validées remontent dans la recherche et les alertes.
- **D967 (acceptée 2026-11-17)** : base **privée par org**, sans base commune entre
  orgs (Q-PD2) : elle franchirait la frontière de l'org (modèle de menace de
  `security.md`). Une base commune demanderait son propre PRD.
- Indicateurs par pièce et par modèle : retours, corrections, confiance de
  la nomenclature (part des lignes validées par une personne parmi les
  lignes de la nomenclature, et nombre de retours sur 12 mois).

### 5.7 Hors ligne (D968)
- **D968 (acceptée 2026-11-17)** : synchronisation des appareils d'un site ou d'une
  tournée dans l'app mobile (fiches, nomenclatures, vues éclatées,
  procédures, PDF compressés).
- Photos de plaque prises hors ligne : OCR serveur à la synchronisation,
  résultat renvoyé en proposition (§4.2).
- Actions hors ligne (photos, notes, panier) en file, synchronisées au
  retour du réseau, en concurrence optimiste (version lue).
- Phasage : la **file d'envoi** (photos de plaque ou de pièce, notes) arrive
  dès PD1, c'est le minimum du scénario de plaque ; la **consultation** hors
  ligne d'un site ou d'une tournée (fiches, nomenclatures, PDF) reste en PD5.

### 5.8 Validation des propositions
- Tout ce qui est extrait ou proposé (nomenclatures, repères, équivalences,
  remplacements, corrections, notes terrain) entre en proposition.
- Permission IAM `maintenance:validate`, détenue par défaut par le rôle
  éditeur ; une org la restreint à ses référents par un rôle personnalisé.
- L'auteur d'une proposition ne la valide pas lui-même (vérifié côté
  serveur).

## 6. Modèle de données

Pas de tables métier (D960). Ce qui s'ajoute :

| Où | Quoi |
|---|---|
| Pack `maintenance` | types `equipment_model`, `part_reference`, `part_correction` ; liens `manufactures`, `instance_of`, `refers_to`, `supersedes`, `equivalent_to`, `compatible_with`, `bom_line`, `documents` ; extension de `spare_part` (famille, caractéristiques) et des types d'équipement (série, date de fabrication) |
| Noyau | table `qr_tags` (D962) ; index des références normalisées (D961) ; embeddings dans l'index de `doc-search.md` |
| Médiathèque | PDF, images (RustFS) ; ancres = calques d'annotations |
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
| Assistant | Lecture (recherche de pièce, nomenclature filtrée d'un appareil, remplacements, fiche pièce) ; écritures en proposition seulement : ajout au panier d'un OT, correction de pièce, via le service partagé avec l'UI |
| Moteur de pipelines, flows | Ingestion et validation |
| Propositions | Validation humaine de tout ce qui est extrait ou proposé |
| Agents navigateur | Module prix §9 |
| Studio 360 | Localisation des appareils |
| Devices et capteurs | Mesures sur la fiche ; plus tard : panne → pièce probable |

## 8. Phasage

| Lot | Contenu | Critère de sortie |
|---|---|---|
| **PD0 — Socle** | Extension du pack `maintenance`, import CSV de nomenclatures, recherche par référence normalisée avec remplacements, fiche pièce simple | Une référence ancienne renvoie l'actuelle en < 300 ms sur 100 000 pièces |
| **PD1 — QR et parc** | Jetons QR, planches vierges, association sur place, fiche appareil, file d'envoi hors ligne (photos, notes), nomenclature filtrée par série, date et variante, panier = OT, export CSV du panier | Du scan à la fiche pièce en < 10 s sur mobile |
| **PD2 — Doc riche** | Viewer PDF ancré, vue éclatée à hotspots (saisie manuelle), procédures de montage en pages Pages (L6 plus tard), notes terrain | Le technicien trouve la procédure sans quitter PNEX |
| **PD3 — Ingestion assistée** | Extraction nomenclatures et repères depuis les PDF, écran de relecture | Vue éclatée de 50 repères intégrée en < 10 min de relecture |
| **PD4 — Multimodal** | OCR de plaque côté serveur (différé si hors ligne), recherche par photo de pièce | Plaque reconnue dans > 80 % des cas sur un jeu de test |
| **PD5 — Apprentissage et hors ligne** | Corrections, hors ligne mobile | Une correction validée apparaît dans la recherche et les alertes de l'org |

PD1 recoupe les lots C-QR1 et C-QR2 de l'annexe C : ils se font une fois.

## 9. Extension liée (PRD séparé) : prix et fournisseurs (D969)
**D969 (acceptée 2026-11-17)** : module à part, branché sur le panier :
- récupération **à la demande** par un agent navigateur, avec le compte du
  client dans le coffre de secrets ;
- cache court par org (prix, disponibilité), cache long des
  correspondances référence ↔ modèle enrichi par les corrections ;
- comparaison entre fournisseurs (lien `supplies` de l'annexe C), marges,
  relance quand un prix est trop ancien ;
- aucune aspiration de catalogue en tâche de fond, rythme limité, respect
  des conditions d'utilisation (garde de sortie R8).

## 10. Juridique et données (D970–D971)
- **D970 (acceptée 2026-11-17)** : un document importé par une org reste **privé à
  cette org** ; rien ne la quitte (pas de base commune, D967).
- PDF fabricants jamais republiés ; une vue éclatée protégée se cite
  par lien vers sa source.
- **D971 (acceptée 2026-11-17)** : une page QR publique ne contient aucune donnée
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

## 12. Questions tranchées (2026-11-17)
1. **Q-PD1** Sources de nomenclatures : découvertes avec le pilote. PD0
   couvre l'import CSV générique, PD3 l'extraction PDF ; la liste des
   fabricants reste hors dépôt.
2. **Q-PD2** Base commune entre orgs : **non**, base privée par org (D967).
3. **Q-PD3** Étiquettes : QR seul, matière au choix du client, pas de NFC
   (§5.4).
4. **Q-PD4** Variantes : une ligne de nomenclature est bornée par série,
   date et variante facultative ; pas de modèle enfant (§3).
5. **Q-PD5** Export du panier : CSV en PD1 ; un PDF seulement si le pilote
   le demande. Pas de connecteur vers une GMAO tierce.
6. **Q-PD6** Pilote : choisi hors dépôt.
7. **Q-PD7** 3D : **retirée** du PRD ; les vues éclatées à hotspots
   suffisent.
8. **Q-PD8** OCR de plaque : côté serveur seulement, différé au retour du
   réseau ; modèle local du serveur, LLM de l'org en option (D119). Un
   modèle embarqué sur le mobile ne viendra que si le pilote montre que
   l'attente gêne.
9. **Q-PD9** Pas de pack `parts_docs` : tout entre dans `maintenance`.

### Éclaircissements (2026-11-17)
- **Procédures** : PD2 sans Pages L6 ; procédure = page Pages liée au
  modèle × pièce, le pas-à-pas vient avec L6 (§5.3).
- **Hors ligne** : file d'envoi dès PD1, consultation hors ligne en PD5
  (§5.7).
- **Validation** : permission `maintenance:validate`, rôle éditeur par
  défaut, pas d'auto-validation (§5.8).
- **Propriétés existantes** (choix par défaut, bêta sans compatibilité) :
  les champs texte `manufacturer` et `model` des types d'équipement de
  `maintenance` sont remplacés par les liens `manufactures` (organisation)
  et `instance_of` (`equipment_model`) ; `serial` existe déjà et reste.
- **Page QR publique** : reportée après PD1 ; en PD1, login ou 403 (§5.4).
- **Assistant** : lecture des pièces, nomenclatures et remplacements ;
  écritures (panier d'OT, correction) en proposition via le service partagé
  avec l'UI (§7, règle `ai-assistant.md` §9.3).
- **Voix** : retirée du mode « mains sales ».
- **Photo de pièce** : modèle d'embeddings choisi en PD4 (§5.1).
- **Référence fabricant de `spare_part`** (choix par défaut) : les champs
  texte `manufacturer` et `manufacturer_ref` sont remplacés par le lien
  `manufactures` et par les objets `part_reference` de nature « origine » ;
  `part_number` (référence interne de l'org) reste.

## 13. Avancement

### PD0 — Socle (codé le 2026-11-19)

- **Pack `maintenance` 1.7.0** : types `equipment_model` (code unique, famille, variantes), `part_reference` (référence normalisée unique, nature origine / distributeur / ancienne, source ; le titre garde l'écriture d'origine), `part_correction` (modèle, plage de séries, référence fausse et bonne, statut) ; liens `instance_of` (tout équipement → modèle), `refers_to`, `supersedes`, `equivalent_to`, `compatible_with` (conditions), `bom_line` (repère, quantité, série de/à, date de/à, variante) ; `manufactured_on` sur les équipements ; `spare_part` gagne famille et caractéristiques et perd les champs texte fabricant (éclaircissements du §12). **`maintenance_partners` 1.1.0** : lien `manufactures` (organisation → modèle ou pièce), là où vit `organization`.
- **Moteur, générique** : option de propriété texte `normalize: reference` (lettres et chiffres en majuscules, appliquée à toute écriture : formulaire, import, flow, assistant ; une valeur sans lettre ni chiffre est refusée) et filtre `starts_with` d'`ontology_query` (sur la valeur telle que stockée). Aucune table métier ni route propre (D960).
- **Imports livrés** (D301, fichier CSV de l'org) : « Parts lists (CSV) » (`model, model_name, position, part_number, part_name, reference, quantity, serial_from, serial_to, date_from, date_to, variant`) et « Part supersessions (CSV) » (`old_part_number, new_part_number, since`), avec aperçu avant écriture.
- **Écran** `/parts` (menu Opérer, « Pièces détachées ») : un champ, l'écriture normalisée affichée sous le champ, les pièces désignées par les références qui commencent ainsi, leurs références et « Remplacée par » la plus récente (5 remplacements suivis au plus). Le bouton d'ajout ouvre les imports. Fiche pièce = la fiche objet générique.
- **Acceptation mesurée** (`tests/maintenance_parts.rs::an_old_reference_finds_the_current_part_among_100000`, ignoré par défaut) : 100 000 pièces et références, une référence ancienne tapée avec séparateurs mène à la pièce actuelle en **258 ms** (build de débogage, deux requêtes HTTP, meilleur de trois).
- **Écarts** :
  - les règles de préfixe fabricant en YAML (§5.1) ne sont pas codées : la normalisation retire seulement séparateurs et casse ;
  - le préfixe est lu sans index dédié (balayage des références de l'org) ; un index d'expression viendra si une base réelle le demande ;
  - l'écran nomme les clés du pack (exception D441 consignée dans `tests/engine_genericity.rs`) ; une recherche par référence déclarée par le pack la lèverait ;
  - l'écran lit les liens de chaque pièce trouvée (20 par page) : `shortcut:` dans le code ;
  - lien `documents` (modèle → média) et `maintenance:validate` reportés à PD2 et aux propositions de PD1.

### PD1a — Étiquettes QR (codé le 2026-11-20, nuit)

- **Noyau** : table `qr_tags` (migration 000158 : jeton aléatoire de 26 caractères base32, org, objet facultatif, état `blank` / `linked` / `revoked`, planche), `pnex_core::qr`, `services::qr_tags`, routes `/api/v1/qr-tags` (liste filtrable par objet, état ou jetons ; planche vierge de 1 à 120 ; étiquette d'un objet, créée au premier usage ; lecture d'un jeton ; liaison d'une étiquette vierge ; révocation). Permissions `qr-tags:read` (lecteur) et `qr-tags:write` (membre) ; MCP : tout exposé sauf la révocation (destructive).
- **R1** : un jeton se lit dans l'org de l'appelant seulement ; celui d'une autre org est inconnu (404), jamais un indice de son existence. Test `tests/qr_tags.rs`.
- **Adresse imprimée** : `<serveur>/q/<jeton>` (l'adresse du serveur de l'app, mobile compris). Un téléphone l'ouvre avec son appareil photo ; sans session, la coquille demande la connexion (D964, phase PD1).
- **Écrans** : `/q/:token` (étiquette liée → sa fiche ; vierge → choix de l'objet par son nom et « Lier ici » ; révoquée ou inconnue → message), « Code QR » sur toute fiche objet avec impression, page « Étiquettes QR » (menu Opérer : planche vierge, liste paginée, révocation), planche imprimable `/qr-sheet` hors de la coquille (4 × 10 par A4, seul le contenu s'imprime).
- **Reste de PD1** : scanner intégré à l'app (caméra), nomenclature de l'appareil filtrée par série, date et variante, panier = OT et son export CSV, file d'envoi hors ligne (photos, notes).
