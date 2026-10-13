# Étude « Calendrier GMAO »

Phase de recherche de `docs/architecture/ontology.md` annexe B (roadmap
P2.20, décision #26). Ce dossier contient des **notes**, pas du code.

Règles clean-room (annexe B2), opposables :

- aucun extrait ni pseudo-code tiré d'une source copyleft ; on décrit des
  comportements, jamais des implémentations ;
- schémas tiers au niveau concept seulement (jamais les noms de tables ni
  de colonnes) ;
- captures dans `<projet>/shots/` : **non versionnées** (`.gitignore`) ;
- le code PNEX s'écrit depuis le PRD, pas depuis ces notes.

| Fichier | Contenu |
|---|---|
| `atlas-cmms.md`, `odoo-maintenance.md`, `erpnext.md`, `openmaint.md` | GMAO : modèle métier |
| `eventcalendar.md`, `fullcalendar.md`, `frappe-gantt.md` | UI calendrier |
| `moteur-recurrence.md` | `rrule`, `icalendar`, RustiCal, contraintes Timefold |
| `matrice.md` | grille comparative (B7) |
| `patterns.md` | patterns retenus et anti-patterns |
| `modele-cible.md` | modèle PNEX proposé (B8 tranché) et questions |

État au 2026-10-13 : **étude documentaire faite** (docs, démos, tickets
publics, aucune instance lancée). Restent les passages en local des
scénarios S1–S6 (« à jouer » dans les fiches) et les maquettes.
