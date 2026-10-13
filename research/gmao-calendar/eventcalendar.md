# EventCalendar (vkurko/calendar) — fiche d'analyse

> Étude documentaire du 2026-10-13 : README, démos, npm, `LICENSE` lu.
> Rien n'a été exécuté.

## Identité

- Version 5.17.1 (2026-10-09). **MIT** vérifiée.
- Svelte 5 compilé, sans dépendance. Bundle standalone d'environ 133 Ko
  bruts (environ 35 Ko en brotli), plus 15 Ko de CSS.
- Utilisable sans framework : `<script>`, puis création sur un élément
  hôte et destruction explicite. C'est le candidat d'embarquement le
  plus direct pour Dioxus.

## Vues

- Mois et semaine en grille de jours (avec « +n »).
- Agenda horaire jour et semaine.
- Listes (jour à année), la bonne vue pour le mobile.
- Une colonne par ressource (jour, semaine).
- **Timeline de ressources** (jour à année) avec ressources imbriquées
  et groupes.
- Pas de Gantt ni de dépendances.

## Interactions

- **Drag et resize** (aussi depuis le début), avec un pas
  d'accrochage.
- **Glisser d'une ressource à une autre** : l'ancienne et la nouvelle
  ressource sont fournies.
- **Sélection de créneau** pour créer.
- **Contraintes** de drag, resize et sélection exprimées par des
  fonctions : l'élément s'arrête à la dernière position permise. Il n'y a
  pas de règle de chevauchement déclarative, le conflit se code.
- **Événements de fond.**
- **Absents** : heures ouvrées (seulement des jours mis en évidence et
  des heures min/max), multi-sélection.

## Performance, accessibilité, mobile, i18n, thème

- **Performance** : gains récents sur la grille et la timeline, pas de
  virtualisation documentée. Le chargement se fait par plage (sources
  sous forme de fonction).
- **Accessibilité** : faible, rien sur le clavier.
- **Tactile** : appui long.
- **i18n** : par `Intl`, fr-FR natif.
- **Thème** : variables CSS, mode sombre par classe. S'aligne sur les
  jetons PNEX.

## Scénarios

| # | Constat |
|---|---|
| S3 | Glisser une occurrence ; la notion « occurrence d'une série » reste au serveur (pas de RRULE natif) |
| S5 | Timeline sur 4 semaines, une ligne par technicien, groupes par équipe, glisser entre lignes ; jauge de charge à calculer et afficher dans l'étiquette de ressource |
| S6 | Arrêt en événement de fond ; pas de dépendances |

## Points forts / faiblesses / idées

- **Forces** : MIT intégral, y compris la timeline de ressources ;
  léger ; API proche de FullCalendar ; thème par variables.
- **Faiblesses** : accessibilité, pas de virtualisation, mainteneur
  quasi unique.
- **Idée** : embarquer en IIFE comme ProseMirror et maplibre
  (`pages.md` P3). Garder la passerelle étroite : événements,
  ressources, callbacks drop, resize et select. Le serveur valide
  conflits et contraintes, avec des refus 409 à code machine. Ajouter une
  couche d'accessibilité minimale côté PNEX.

## Notes (0–3)

| Critère | Note |
|---|---|
| Vue mois/semaine | 3 |
| Ressource/timeline | 2 |
| Gantt | 0 |
| Glisser-déposer | 3 |
| Conflits | 1 |
| Filtres/densité | 2 |
| Mobile | 2 |
| Perf | 2 |

## Verdict

**À reprendre (embarqué)** pour les vues mois, semaine, liste et la
timeline de ressources.

## Sources

https://github.com/vkurko/calendar (LICENSE, releases) ·
https://registry.npmjs.org/@event-calendar/core
