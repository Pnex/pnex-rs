# Atlas CMMS — fiche d'analyse

> Étude documentaire du 2026-10-13 (README, site, tickets publics). Aucun
> code consulté (AGPL). Les points « à jouer » attendent le passage en
> local (`ontology.md` annexe B4).

## Identité

- Version étudiée : v1.9.0 (2026-09-22), dépôt actif.
- Licence vérifiée : **AGPLv3** (et non GPLv3), plus une licence
  commerciale pour la marque blanche et certaines fonctions avancées.
- Stack (contexte) : Java/Spring Boot, React/TypeScript, mobile React
  Native ; ordonnanceur de jobs côté serveur.

## Modèle métier observé

- **Entités** : OT ; plan préventif (gabarit qui produit des OT) ;
  demande d'intervention (par QR code ou URL) ; équipement (hiérarchie,
  statuts) ; lieu ; compteur et relevés ; pièce/stock ; achat ; équipe ;
  utilisateur ; prestataire ; checklist. Un plan vise un équipement, un
  lieu ou une équipe ; les OT générés remontent dans l'historique de
  l'équipement.
- **Cycle de vie d'un OT** : filtres mobiles par statut, priorité et
  assigné, regroupements « en retard / aujourd'hui / en cours ». Noms
  des statuts : à jouer. Une signature exigée par le plan n'était pas
  reprise par les OT générés (ticket #216).
- **Déclencheurs** : date (jour, semaine, mois, an avec une fréquence
  N) ; compteur (relevés périodiques, statut « dû » quand un relevé est
  attendu, graphe) ; « événement » annoncé mais non documenté ;
  déclenchement manuel d'un plan.
- **Matérialisation** : **à la volée**. Un job crée l'OT à l'échéance,
  avec un e-mail « OT à venir » avant. Aucune occurrence future stockée.
  Le ticket #297 montre une rafale d'OT au redémarrage et des échéances
  ratées jamais rattrapées : l'état de planification vit dans
  l'ordonnanceur, sans réconciliation.

## UX calendrier

- Un calendrier unique mêle OT, plans à venir et demandes ; glisser
  change l'échéance.
- Vue **charge** (depuis v1.7) : charge par personne sur une période,
  liste des OT non planifiés, affectation avec une date de début
  estimée, motifs de postes.
- Surcharge et conflits : seuls les postes sont gérés ; exclusion des
  week-ends demandée (#63), sans suite connue.
- Mobile : écran « Ma journée », hors ligne (offre payante), saisie des
  relevés, notifications push.

## Scénarios

| # | Constat |
|---|---|
| S1 | Mensuel avec fréquence 3 ; date d'ancrage à jouer |
| S2 | Compteurs et relevés existent ; seuil « tous les N » et cumul à jouer |
| S3 | Pas d'occurrence future : seul l'OT déjà généré se déplace |
| S4 | Une option « récurrence depuis la date planifiée » existe (#297) ; la variante depuis la clôture est probable, non vérifiée |
| S5 | Vue charge sur une plage de dates ; réaffectation par glisser à jouer |
| S6 | Ni dépendance ni arrêt planifié : non couvert |

## Points forts (à reprendre)

- Un calendrier unique et un glisser simple.
- Une vue de charge avec la file « non planifiés ».
- Les relevés de compteur intégrés au parcours terrain (statut « dû »,
  graphe).
- Le déclenchement manuel d'un plan ; la duplication et l'import tableur
  des plans.

## Faiblesses (à éviter)

- Une planification portée par des timers persistés : non idempotente au
  redémarrage, sans rattrapage.
- Des occurrences futures invisibles, impossibles à modifier.
- Des exigences du plan qui ne passent pas à l'OT.
- Le droit « voir » suffit pour déclencher un plan (#268, ouvert).
- Pas de dépendances, de Gantt ni de jours ouvrés.

## Idées pour PNEX

- Occurrences **calculées** de façon déterministe depuis la règle, sur un
  horizon, plutôt que des timers.
- Réconciliation idempotente au démarrage : l'upsert `time_ranges` par
  `external_id` le permet déjà.
- Une exception par occurrence (S3) et un choix fixe/flottant explicite.
- Le gabarit porte tout ce que l'OT exige.
- Les compteurs viennent de la **télémétrie** et non de la saisie : c'est
  l'avantage naturel de PNEX.
- Déclencher un plan est une écriture : garde de rôle (R2).

## Notes (0–3)

| Critère | Note | Commentaire |
|---|---|---|
| Récurrence | 2 | j/s/m/a × N |
| Compteur / condition | 1 | relevés manuels |
| Fixe vs flottant | 1 | présumé |
| Matérialisation | 1 | à la volée, fragile |
| Vue mois/semaine | 2 | unifiée |
| Ressource/timeline | 2 | vue charge |
| Gantt | 0 | — |
| Glisser-déposer | 2 | échéance seulement |
| Conflits | 1 | postes seulement |
| Filtres/densité | 1 | à jouer |
| Mobile | 3 | journée, hors ligne, relevés |
| Interop ics/CalDAV | 0 | — |
| Perf | 1 | rien sur le calendrier |

## Verdict

Complet sur les objets métier et le terrain, superficiel sur la
planification. **À reprendre** : l'UX terrain et la vue de charge.
**À éviter** : le moteur de récurrence.

## Sources

- https://github.com/Grashjs/cmms (README, LICENSE, releases v1.7.0 et v1.9.0)
- https://www.atlas-cmms.com (pages features/preventive-maintenance, features/work-orders)
- Tickets #47, #63, #88, #172, #174, #216, #242, #243, #248, #255, #268, #297
