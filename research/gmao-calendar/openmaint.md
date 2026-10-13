# openMAINT — fiche d'analyse

> Étude documentaire du 2026-10-13 : site éditeur, changelog public, doc
> CMDBuild, forum. Notes de version détaillées derrière un login. Aucun
> code consulté.

## Identité

- Version 2.4 (2025-04-30), déclinaison de CMDBuild.
- Licence **AGPL** (site éditeur, sans numéro de version : à confirmer).
- **App mobile et portail self-service propriétaires** (abonnement).

## Modèle métier observé

- **Patrimoine** : bâtiments, étages, locaux, unités, espaces
  extérieurs ; actifs techniques ; mobilier.
- **Préventif** : un **manuel de maintenance** (référentiel des
  fréquences et des check-lists) appliqué par un plan à un sous-ensemble
  d'actifs.
- **Correctif** :
  - chaîne signalement (alarme, usager, bureau) → classification →
    approbation → devis → OT ;
  - exécution interne ou externe, avec durées, matériels et coûts ;
  - depuis 2.3, **une anomalie relevée en préventif ouvre un correctif
    automatiquement**.
- **Déclencheurs** :
  - temps ;
  - **compteur** ;
  - **relevés automatiques par Modbus** (2.2).
  
  La condition sur seuil n'est pas confirmée.
- **Matérialisation** :
  - calendrier préventif généré automatiquement, avec un **délai
    d'alerte** (par exemple 14 jours) ;
  - OT futurs visibles ;
  - planificateurs paramétrables (2.4).
  
  Horizon non observé. Le forum signale des générations qui ne
  partent pas.

## UX calendrier

- Page calendrier préventif et widget filtrable.
- **Gantt** de planification du préventif, avec des couleurs pour annulé
  et interrompu.
- Glisser et charge par équipe : non observés.
- Mobile : check-lists, scan QR (app propriétaire).

## Spatial (pertinent pour les POI et la carte PNEX)

- Actifs géoréférencés sur OpenStreetMap et sur des plans 2D vectoriels
  (import DWG semi-automatique, lien élément de plan ↔ fiche).
- IFC/BIM avec visualiseur 3D.
- Fiche bâtiment ouverte depuis la carte.
- **Signalement d'un correctif en choisissant le local sur la carte**
  (2.4).

## Scénarios

| # | Constat |
|---|---|
| S1 | Oui (fréquence du manuel) |
| S2 | Oui en principe (compteur) ; paramétrage non public |
| S3 | À jouer |
| S4 | **Recalage de la maintenance planifiée selon le retard ou l'avance de la dernière exécution** (2.2) : le flottant existe ; le choix fixe/flottant n'est pas observé |
| S5 | Le Gantt donne une vue d'ensemble ; la charge par équipe n'est pas observée |
| S6 | Non observé |

## Points forts / faiblesses / idées

- **Forces** : la couverture la plus complète (temps, compteur,
  recalage, Gantt) et l'ancrage spatial.
- **Faiblesses** : configuration lourde, doc publique pauvre, mobile
  non libre, UX datée.
- **Idées** :
  - signaler une intervention **depuis le POI sur la carte** ;
  - recalage explicite d'après la dernière exécution ;
  - délai d'alerte avant échéance ;
  - un contrôle préventif hors tolérance ouvre un correctif (mesure P8
    des Pages → OT) ;
  - compteurs alimentés par la télémétrie (PNEX l'a déjà, sans Modbus
    dédié).

## Notes (0–3)

| Récurrence | Compteur | Fixe/flottant | Matérialisation | Mois/semaine | Ressource | Gantt | Glisser | Conflits | Filtres | Mobile | Interop | Perf |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2 | 2 | 2 | 2 | 2 | 1 | 2 | — | — | 2 | 1 | — | — |

## Verdict

**À reprendre** : le « quoi » (compteur, recalage, Gantt, spatial,
correctif né du préventif). **À éviter** : le « comment » (configuration
lourde).

## Sources

- tecnoteca.com/en/products/openmaint
- openmaint.org/en/download/changelog
- docs.cmdbuild.org (vertical-solutions/openmaint)
- forum.cmdbuild.org : fils 4680 et 3890
