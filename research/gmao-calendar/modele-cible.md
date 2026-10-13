# Modèle cible PNEX (B8 tranché, proposition)

> Proposition issue de l'étude documentaire du 2026-10-13. Elle sera
> soumise à la relecture et alimentera le PRD « Calendrier GMAO », qui
> attribuera les numéros D. Principe directeur : **réutiliser ce qui
> est livré**. Le noyau ne gagne que ce qui sert ailleurs, et la GMAO
> est un pack (D190).

## 1. Ce que le noyau gagne

| Ajout | Pourquoi au noyau | Détail |
|---|---|---|
| Type de propriété `recurrence` (D178) | sert aussi aux plages média, aux postes d'équipe, aux rapports périodiques | valeur = `{rrule, dtstart, tzid}`. Validation et énumération **bornée** dans `pnex-core` (même code front et back), fuseaux IANA seuls, revalidée côté serveur |
| Vue calendrier des plages | `pages.md` §10 et les dossiers (`cases.md`) la veulent aussi | lit `time_ranges` de n'importe quelle portée ; mois, semaine, liste, timeline de ressources |
| Flux `.ics` en lecture seule | interop absente partout, attendue | jeton révocable par utilisateur × org (R4 : jamais en clair dans un DTO de lecture) ; pas de CalDAV en V1 |

## 2. Ce que le pack Maintenance gagne (YAML, version 1.2.0)

- **Type `maintenance_plan`**, avec ses propriétés :
  - `trigger` (enum `date` | `meter`) ;
  - `recurrence` (si `date`) ;
  - `meter` (réf. vers la propriété `series` de la cible) et
    `meter_every` (nombre, si `meter`) ;
  - `mode` (enum `fixed` | `floating`) ;
  - `lead_days`, `tolerance_days` ;
  - `immobilizes` (booléen) ;
  - `procedure` (réf. de page modèle, plus tard).

  Lien **`maintains`** : plan → cible, de n'importe quel type, **device
  compris**. Un plan sur un device seul marche donc sans aucun autre type
  du pack : l'identité universelle (annexe A2) le permet déjà.
- **Types `team`** (et compétences plus tard) ; un technicien est un
  utilisateur de l'org (voir Q3).

## 3. Occurrences = plages (D182), pas de table neuve

Une occurrence est une ligne `time_ranges` à portée objet (la **cible**),
qui reprend les briques existantes :

| Champ existant | Usage |
|---|---|
| `external_id` | `plan:<uuid>:<date>` : l'upsert idempotent fait la **réconciliation** (pattern 2) |
| `category` | `maintenance` (préventif) ou `corrective` |
| `planned_start/end` | fenêtre prévue, déplacée par l'utilisateur en cas d'exception |
| `actual_start/end` | exécution réelle ; sa fin nourrit le mode flottant |
| `origin` | `grid` (généré par le plan), `manual`, `detected` (créé par un flow) |
| `source_ref` | `plan:<uuid>@<version>` (provenance D184) |
| `attrs` | `{plan_id, status, ready, ideal, due, assignee, exception}` |

- **Statut** : `planned` → `ready` (J-`lead_days`) → `in_progress` →
  `done` | `cancelled`. `late` est **calculé** (échéance dépassée et pas
  `done`), jamais stocké.
- **Exception** : `attrs.exception = moved | cancelled`. Le matérialiseur
  ne touche jamais une occurrence en exception ni une occurrence passée.
- **Matérialiseur** : une tâche Loco périodique (**un seul pod**,
  singleton existant) projette chaque plan sur l'horizon et fait les
  upserts. Elle est rejouable à volonté.
  - En mode fixe, la projection part de `dtstart`.
  - En mode flottant, elle part de `actual_end` de la dernière
    occurrence `done`.
- **Compteur** : le matérialiseur lit l'**accroissement** de la série
  depuis la dernière réalisation (agrégat `increase` déjà livré). Une
  occurrence naît quand le seuil est atteint. Une **date prévisionnelle**
  (tendance linéaire, puis nœud forecast) s'affiche en pointillé, avec
  `origin=detected`.
- **Condition** : rien de neuf. Un flow (anomalie, seuil) crée
  l'occurrence corrective avec le nœud **`range_upsert` existant**.

## 4. OT, actions, assistant

- **OT = instance de procédure des Pages (P9, lot L6)**, attachée à
  l'occurrence. Tant que L6 n'existe pas, l'occurrence porte elle-même le
  minimum : assigné, statut, notes et mesures de clôture (voir Q1).
- **Au début d'une occurrence `immobilizes`**, l'UI propose l'**action
  D183** `start_maintenance`, pré-remplie. Un flow déployé décide de ce
  qui se passe physiquement (D123). Le calendrier ne commande jamais un
  device.
- **Assistant** : il propose, il n'écrit pas en silence.
  - Outils de proposition : `propose_plan`, `propose_reschedule`, sur le
    modèle de `propose_action`, avec un bouton « Vérifier et
    enregistrer ».
  - Lecture : charge d'une équipe, occurrences en retard.
  - Une fiche KB par fonctionnalité.

## 5. UI

| Vue | Choix |
|---|---|
| Mois, semaine, liste, multi-mois, timeline de ressources (S3, S5) | **EventCalendar embarqué en IIFE** (MIT, environ 35 Ko), passerelle étroite (événements, ressources, callbacks drop, resize, select), thème par jetons PNEX, couche d'accessibilité PNEX |
| Gantt d'arrêt technique (S6) | **SVG Dioxus natif** ; dépendances et cascade calculées côté serveur (`petgraph`) |
| Éditeur de plan | aperçu des 10 prochaines dates (pattern 10), choix fixe/flottant expliqué |
| Terrain | « Ma journée » + page objet (onglet Maintenance) ; hors ligne : Q5 |
| Carte | signaler une intervention depuis un POI (pattern 17) |

## 6. Sécurité (grille §6)

- Toute écriture (plan, déplacement, clôture, génération manuelle) passe
  par `can_write` et `write_role` du type. Un viewer reçoit 403 (R2).
  L'org vient du principal (R1).
- Récurrence bornée en nombre et en horizon : pas de règle qui fait
  tourner le serveur.
- Jeton `.ics` : révocable, haché en base, portée lecture seule sur
  l'org.
- Plafond de débit sur les invocations d'actions (écart D183 déjà noté) :
  à régler avant que le calendrier ne les propose en série.

## 7. Questions ouvertes

1. **OT avant les procédures L6** : un OT minimal dans `attrs`, ou
   attendre L6 ? (proposition : `attrs` minimal, puis migration vers
   l'instance de procédure)
2. **Vue calendrier au noyau ou au pack** (B11.6) : proposition noyau,
   car elle sert aussi au média et aux dossiers.
3. **Techniciens** : utilisateurs de l'org (simple, ACL incluse) ou
   objets `person` (externes, prestataires) ? Les deux, via un lien ?
4. **`rrule` vs sous-ensemble maison** : décider après un
   `cargo check --target wasm32` et une mesure du bundle. Repli : aperçu
   calculé par le backend, `pnex-core` réduit aux DTO.
5. **Hors ligne terrain en v1** ? Proposition : non, en ligne d'abord
   (comme les Pages L2).
6. **Calendriers ouvrés** (fériés, postes) : `shift_pattern` existe sur
   `line`. Proposition : jours non ouvrés par org en v1, postes plus
   tard.
7. **Horizon** : 90 jours par défaut, configurable par org ?
8. **Date prévisionnelle du compteur** : tendance linéaire en v1, nœud
   forecast ensuite ?
