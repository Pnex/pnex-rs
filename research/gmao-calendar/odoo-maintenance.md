# Odoo Maintenance (Community) — fiche d'analyse

> Étude documentaire du 2026-10-13 : doc 19.0 et 18.0, un blog
> intégrateur. Aucun code consulté (LGPL).

## Identité

- Version 19.0 ; la doc 18.0 est identique sur ce module.
- Community sous **LGPLv3** (page légale non relue). Les feuilles de
  travail personnalisées dépendent probablement d'Enterprise
  (incertain).
- Stack (contexte) : Python, PostgreSQL, client web Owl, app mobile.

## Modèle métier observé

- **Entités** : équipement ou poste de travail, équipe de maintenance,
  technicien responsable, demande de maintenance.
- **Demande** :
  - type correctif ou préventif ;
  - date planifiée, durée ;
  - priorité 0–3 ;
  - instructions (PDF, diapos, texte).
- **Équipement** : **MTBF, MTTR et date estimée de la prochaine panne**
  calculés depuis l'historique.
- **Cycle de vie** : étapes kanban, Nouvelle → En cours → Réparée, ou
  Rebut en cas d'échec.
- **Déclencheurs** : date seulement (récurrent, « tous les N
  jours/semaines/mois/ans », sans fin ou jusqu'à une date). Ni
  compteur ni condition en standard.
- **Matérialisation** : **une seule occurrence à la fois**. La suivante
  est créée à la clôture de la courante, donc pas d'horizon. La base de
  calcul (prévue ou clôture) n'est pas documentée.

## UX calendrier

- **Vues** : jour, semaine (par défaut), mois, année, avec ou sans
  week-end. Aussi kanban, liste et pivot.
- **Création et déplacement** : clic sur le calendrier pour créer ;
  glisser documenté en kanban seulement.
- **Blocage** : « bloquer le poste » interdit toute autre activité sur
  l'équipement pendant l'intervention. Pas de détection de surcharge.
- **Filtres** : favoris partagés.
- **Mobile** : web responsive et app.

## Scénarios

| # | Constat |
|---|---|
| S1 | Tous les 3 mois, probable (non vérifié) |
| S2 | Non couvert en standard (module communautaire non étudié) |
| S3 | On modifie la date de la demande ; effet sur la suivante à jouer |
| S4 | Fixe ou flottant : à jouer |
| S5 | Calendrier/pivot filtré par équipe ; réaffecter = changer le responsable ; pas de vue de charge |
| S6 | Pas de dépendances ; seul le blocage de poste |

## Points forts / faiblesses / idées

- **Forces** : simplicité ; indicateurs de fiabilité calculés.
- **Faiblesses** : une seule occurrence future, donc aucune vision à N
  mois. Pas de compteur, de Gantt ni de charge.
- **Idées** :
  - MTBF, MTTR et prochaine panne estimée, calculés depuis l'historique
    des OT (et, chez PNEX, depuis les séries) ;
  - un drapeau « immobilise l'équipement » sur l'intervention ;
  - le kanban pour le correctif.

## Notes (0–3)

| Récurrence | Compteur | Fixe/flottant | Matérialisation | Mois/semaine | Ressource | Gantt | Glisser | Conflits | Filtres | Mobile | Interop | Perf |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 1 | 2 | 0 | 0 | 1 | 1 | 2 | 2 | 0 | — |

## Verdict

**À reprendre** : l'UX kanban du correctif et les indicateurs de
fiabilité. **À éviter** : le modèle « une occurrence à la fois ».

## Sources

- odoo.com/documentation/19.0 : maintenance_requests, add_new_equipment
- odoo.com/documentation/18.0 : maintenance_calendar
- cybrosys.com : préventif sous Odoo 19
