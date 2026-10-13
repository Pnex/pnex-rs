# FullCalendar — fiche d'analyse

> Étude documentaire du 2026-10-13. Le périmètre premium n'a été observé
> que dans la doc et les démos publiques ; aucun code premium n'a été lu.

## Identité

- Version 7.1.2 (2026-10-12). La v7 est récente et son API bouge.
- **Standard : MIT** vérifiée.
- **Premium** (timeline de ressources, impression) : triple licence,
  commerciale, CC BY-NC-ND 4.0 ou GPLv3. **Incompatible avec la
  distribution MIT de PNEX.** Un humain doit lire la licence commerciale
  avant tout achat éventuel.
- TypeScript avec Preact interne. Bundle global d'environ 182 Ko gzip,
  plus le CSS du thème. Utilisable sans framework.

## Vues (MIT)

- Mois, agenda jour/semaine, liste.
- **Multi-mois** : utile pour le plan préventif annuel.

## Vues premium (doc seule)

- Timeline de ressources avec colonnes (équipe, compétence…), groupes
  et imbrication.
- Une colonne par ressource.
- Impression.
- Pas de Gantt.

## Interactions

- Drag, resize et sélection complets.
- **Dépôt depuis une liste externe** : idéal pour planifier un OT depuis
  un backlog.
- Contraintes **déclaratives** :
  - chevauchement autorisé ou non, par booléen, par fonction ou par
    événement ;
  - contrainte de sélection ;
  - **heures ouvrées**, par ressource en premium.
- Événements de fond.
- Pas de multi-sélection.

## Performance, accessibilité, mobile, i18n, thème

- **Performance** : virtualisation seulement sur la timeline premium.
- **Accessibilité** : la meilleure des trois :
  - navigation au clavier ;
  - événements focalisables ;
  - ARIA ;
  - rôles de table pour les lecteurs d'écran.
- **Tactile** : documenté.
- **i18n** : locale fr.
- **Thème** : palettes en v7.

## Scénarios

| # | Constat |
|---|---|
| S3 | Glisser (MIT) ; plugin RRULE, mais exceptions au serveur |
| S5 | La vraie réponse est premium ; en MIT, des calendriers filtrés par équipe seulement |
| S6 | Arrêt en fond, pas de dépendances |

## Points forts / faiblesses / idées

- **Forces** : maturité, accessibilité, contraintes déclaratives, dépôt
  depuis un backlog.
- **Faiblesses** : la planification GMAO est payante ou sous GPL ; le
  bundle est lourd.
- **Idée** : **ne pas embarquer**. Reprendre comme modèle UX, écrit de
  zéro :
  - règle de chevauchement et contraintes ;
  - heures ouvrées par ressource ;
  - dépôt d'un OT depuis la liste « non planifiés » ;
  - colonnes de ressource (équipe, compétence, charge) ;
  - vue multi-mois du préventif ;
  - niveau d'accessibilité visé.

## Notes (0–3, périmètre MIT ; premium entre parenthèses)

| Critère | Note |
|---|---|
| Vue mois/semaine | 3 |
| Ressource/timeline | 0 (3) |
| Gantt | 0 |
| Glisser-déposer | 3 |
| Conflits | 3 |
| Filtres/densité | 2 |
| Mobile | 2 |
| Perf | 1 (3) |

## Verdict

**À améliorer : référence UX seulement**, pas de dépendance.

## Sources

https://github.com/fullcalendar/fullcalendar (LICENSE.md) ·
https://unpkg.com/fullcalendar-scheduler@7.1.2/LICENSE.md ·
https://fullcalendar.io/docs (vanilla-js, timeline-view,
virtual-rendering, eventOverlap, business-hours, accessibility) ·
https://fullcalendar.io/pricing
