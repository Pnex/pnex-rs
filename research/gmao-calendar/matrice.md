# Matrice comparative (annexe B7)

Notes de 0 à 3, issues de l'étude documentaire du 2026-10-13. « — » =
non observé (à jouer en local). Pour FullCalendar, le premium est entre
parenthèses : il est inutilisable pour PNEX (licence).

| Critère | Atlas | Odoo | ERPNext | openMAINT | EventCalendar | FullCalendar | Frappe Gantt |
|---|---|---|---|---|---|---|---|
| Récurrence (RRULE, exceptions, fuseaux) | 2 | 2 | 1 | 2 | — | — | — |
| Compteur / condition | 1 | 0 | 0 | 2 | — | — | — |
| Fixe vs flottant | 1 | 0 | 1 | 2 | — | — | — |
| Matérialisation et horizon | 1 | 1 | 2 | 2 | — | — | — |
| Vue mois/semaine | 2 | 2 | 1 | 2 | 3 | 3 | 0 |
| Ressource / timeline | 2 | 0 | 0 | 1 | 2 | 0 (3) | 1 |
| Gantt / arrêts | 0 | 0 | 0 | 2 | 0 | 0 | 2 |
| Glisser-déposer, replanification | 2 | 1 | — | — | 3 | 3 | 2 |
| Conflits, surcharge | 1 | 1 | 0 | — | 1 | 3 | 0 |
| Filtres, densité | 1 | 2 | 1 | 2 | 2 | 2 | 1 |
| Mobile / terrain | 3 | 2 | 1 | 1 | 2 | 2 | 1 |
| Interop `.ics` / CalDAV | 0 | 0 | — | — | — | — | — |
| Performance perçue | 1 | — | — | — | 2 | 1 (3) | 1 |

Moteur : `rrule` couvre RFC 5545 presque entier, mais sa maintenance
ralentit et sa compilation en wasm32 n'est pas prouvée. `icalendar` est
actif mais sans VTIMEZONE.

## Lecture

- **Personne ne couvre bien les trois couches** à la fois :
  - le modèle métier (openMAINT le plus complet) ;
  - la vue de planification (EventCalendar en MIT) ;
  - l'arrêt technique (Gantt : openMAINT, Frappe).
- **Le compteur est partout faible** : des relevés manuels, sauf Modbus
  chez openMAINT. C'est là que PNEX se distingue, avec des compteurs
  alimentés par la télémétrie.
- **L'interop calendrier est absente** de toutes les GMAO étudiées.
- **Fixe vs flottant n'est jamais un choix explicite** dans l'UI.
