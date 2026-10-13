# ERPNext — Maintenance Schedule / Visit / Asset Maintenance — fiche d'analyse

> Étude documentaire du 2026-10-13 : docs.frappe.io, forum. Aucun code
> consulté (GPL).

## Identité

- Version v16. Code sous **GPLv3**, doc sous CC-BY-SA-3.0.
- Stack (contexte) : Frappe (Python), MariaDB ou PostgreSQL.

## Modèle métier observé

Deux sous-systèmes redondants :

- **Maintenance client** :
  - un **planning** naît d'une commande de maintenance (client,
    articles, intervenant par article) ;
  - périodicité en **liste fermée** (hebdo à annuel, ou « aléatoire ») ;
  - une action « générer » crée **toutes les visites** entre une date de
    début et une date de fin, **visibles avant validation** ;
  - à la validation, des événements apparaissent dans le calendrier de
    l'intervenant ;
  - la **visite réalisée** est une entité distincte (programmée, non
    programmée, panne ; partielle ou complète).
- **Actifs internes** :
  - l'actif doit être déclaré « à maintenir » et avoir une équipe ;
  - une tâche porte un type (préventif, étalonnage), une date de début,
    une périodicité, un assigné et un certificat éventuel ;
  - chaque passage est journalisé avec **quatre statuts : planifié,
    terminé, en retard, annulé** ;
  - une tâche à faire est créée pour l'assigné.
- **Déclencheurs** : date uniquement.
- **Matérialisation** : complète sur l'horizon début–fin côté client.
  Côté actifs, la doc 2026 et le forum 2017 se contredisent.

## UX calendrier

- Événements dans le calendrier générique de l'intervenant.
- Pas de vue ressource ni de Gantt dédié.
- Filtres de liste standard.

## Scénarios

| # | Constat |
|---|---|
| S1 | Oui (trimestriel) |
| S2 | Non |
| S3 | Édition d'une visite générée non documentée : à jouer |
| S4 | Côté actifs, l'échéance suivante part de la **dernière réalisation** (flottant) ; côté client, à jouer |
| S5 | Pas de vue de charge ; réaffecter l'assigné |
| S6 | Non |

## Points forts / faiblesses / idées

- **Forces** :
  - aperçu de l'échéancier avant validation ;
  - statut « en retard » distinct ;
  - consigne : **on clôture ou on annule une occurrence, on ne retouche
    pas le plan**, ce qui préserve l'historique.
- **Faiblesses** : deux modules redondants, fréquences figées, pas de
  compteur, calendrier minimal.
- **Idées** :
  - aperçu des prochaines dates dans l'éditeur de plan ;
  - « en retard » calculé, pas saisi ;
  - preuve exigible à la clôture (certificat, photo, mesure : brique
    P8 des Pages).

## Notes (0–3)

| Récurrence | Compteur | Fixe/flottant | Matérialisation | Mois/semaine | Ressource | Gantt | Glisser | Conflits | Filtres | Mobile | Interop | Perf |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 1 | 2 | 1 | 0 | 0 | — | 0 | 1 | 1 | — | — |

## Verdict

**À reprendre** : la séparation plan → occurrences à statut, et la règle
« on ne retouche pas le plan ». **À éviter** : l'UX calendrier et les
fréquences en liste fermée.

## Sources

- docs.frappe.io/erpnext/user/manual/en : maintenance-schedule, maintenance-visit, asset-maintenance
- docs.frappe.io/erpnext/asset-maintenance-log
- discuss.frappe.io/t/new-feature-asset-maintenance-management/29947
