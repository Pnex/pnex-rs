# Frappe Gantt — fiche d'analyse

> Étude documentaire du 2026-10-13. Bundle MIT mesuré, rien exécuté.

## Identité

- Version 1.2.2 (npm, 2026-02-25). **MIT** vérifiée.
- JS vanilla, rendu SVG, sans dépendance. Environ 13,5 Ko gzip plus 7 Ko
  de CSS.

## Vues

- Échelles de l'heure à l'année, plus des modes personnalisés.
- Une tâche par ligne, sans ressource ni regroupement.

## Interactions

- **Barres** : drag et resize avec accrochage.
- **Avancement** : déplaçable, avec un avancement attendu.
- **Dépendances** : flèches ; déplacer une tâche **déplace ses
  successeurs**. Aucune contrainte appliquée, pas de chemin critique.
- **Non-ouvré** : jours fériés et week-ends exclus du calcul.
- **Lecture seule** : globale ou par aspect.
- **Absents** : sélection de créneau, création au clic.

## Performance, accessibilité, mobile, i18n, thème

- **Performance** : SVG complet, sans virtualisation. Prudence au-delà
  de quelques centaines de tâches.
- **Accessibilité et tactile** : non documentés.
- **i18n** : par `Intl`.
- **Thème** : une vingtaine de variables CSS, mode sombre.

## Scénarios

| # | Constat |
|---|---|
| S3 | Glisser la barre, mais ce n'est pas un calendrier |
| S5 | Non couvert (pas de ressources) |
| S6 | **Point fort** : arrêt parent, 8 OT reliés, cascade au déplacement, fériés de l'usine ignorés |

## Points forts / faiblesses / idées

- **Forces** : minuscule, lisible, dépendances et avancement visibles,
  thémable.
- **Faiblesses** :
  - pas de ressources, de virtualisation ni d'accessibilité ;
  - un seul éditeur, qui avance au rythme de ses propres besoins.
- **Idée** : un Gantt d'arrêt technique, c'est quelques dizaines de
  barres, des flèches et une échelle de temps. Le **réécrire en SVG
  Dioxus** : le front sait déjà faire, avec le canvas des flows et
  l'éditeur de pinout. Les dépendances et la cascade se calculent côté
  serveur en Rust (tri topologique `petgraph`, déjà retenu en annexe A3),
  et le rendu ne fait qu'afficher. L'embarquer en V0 reste possible.

## Notes (0–3)

| Critère | Note |
|---|---|
| Vue mois/semaine | 0 |
| Ressource/timeline | 1 |
| Gantt | 2 |
| Glisser-déposer | 2 |
| Conflits | 0 |
| Filtres/densité | 1 |
| Mobile | 1 |
| Perf | 1 |

## Verdict

**À reprendre comme modèle**, réécrit nativement pour S6.

## Sources

https://github.com/frappe/gantt (license.txt, releases) ·
https://frappe.io/gantt · https://registry.npmjs.org/frappe-gantt
