# Formatage du front : `cargo fmt` + `dx fmt` gardé

> Principe d'hygiène adopté le 2026-10-02, tant que la base de code est
> encore petite : **tout le code du front est formaté par les deux outils,
> et la CI le vérifie.**

## Deux formateurs, deux territoires

| Outil | Ce qu'il formate | Ce qu'il ignore |
|---|---|---|
| `cargo fmt` (rustfmt) | Tout le Rust | L'intérieur des macros `rsx!` (pas du Rust standard) |
| `dx fmt` (dioxus-autofmt 0.7.10) | L'intérieur des `rsx!` : arbre d'éléments, attributs, raccourcis `x: x` → `x`, corps des closures d'événements | Le Rust hors `rsx!` |

Ils ne se contredisent pas : l'ordre `dx fmt` puis `cargo fmt` converge vers
un état accepté par les deux.

## Commandes

- `task fmt` — formate (rsx gardé, puis `cargo fmt --all`) ;
- `task fmt:check` — vérifie sans rien modifier, **même garde que le job
  CI `fmt`** (bloquant).

Ne **jamais** lancer `dx fmt` à la main sur le dépôt :

- il a des bugs de recollage : sur certaines constructions il déplace ou
  duplique des commentaires, perd des tokens (fichier qui ne compile plus,
  ou pire : qui compile avec un commentaire collé au mauvais endroit), ou ne
  converge jamais (ré-indentation infinie) ;
- `dx fmt --check` **réécrit** les fichiers malgré son nom (et sort en 0
  avec `-f`).

Le wrapper `crates/pnex-frontend/scripts/rsx_fmt.py` passe `dx` uniquement
par stdin/stdout et n'écrit un fichier que si le résultat est un point fixe,
garde exactement la séquence des commentaires et garde le flux de tokens
(aux seules normalisations légitimes près : virgules, `r#`, `x: x`,
accolades autour d'un bras de `match` ou d'un corps de closure). Sinon il
laisse le fichier intact et le signale.

## Constructions que `dx fmt` ne digère pas → réécritures

Quand `task fmt` refuse un fichier, la cause est presque toujours l'une de
celles-ci (constatées sur tout le front, 2026-10-02). Elles déclenchent le
bug parce que `dx` doit **réimprimer** un gros morceau de Rust à l'intérieur
d'un `rsx!` ; écrites directement dans la forme qu'il produirait, il n'y
touche plus.

1. **`{match x { A => rsx! {…}, B => rsx! {…} }}` en bloc d'expression** dans
   le balisage. → `if let Some(v) = x { … } else { … }` natif rsx quand c'est
   un `Option` à deux bras ; sinon un `match` au niveau du balisage, sans les
   accolades englobantes. Même chose pour `{ if c { rsx!{…} } else { rsx!{} } }`
   → `if c { … }` natif.
2. **Lecture d'une ressource dans le balisage** (`{match &*res.read() {…}}`)
   → calculer la valeur dans un `let` avant le `rsx!`, puis `if`/`for` natifs.
3. **Expression multi-ligne en valeur d'attribut** (`a || b` sur plusieurs
   lignes, chaîne `.iter().map(…).collect()`) → ne converge jamais. Extraire
   dans une fonction ou un `let`, l'attribut tient sur une ligne.
4. **Corps de handler non canonique** dans une closure d'événement :
   `let P = e else { return; };` ou `if c { return; }` sur une ligne, tuple
   long dans un `let … else`. → les écrire sur plusieurs lignes, comme le
   ferait rustfmt (`else {` / `return;` / `};`).
5. **`sig.with_mut(|s| { … })` multi-ligne** → `sig.write()` :
   `let mut set = expanded.write(); if !set.remove(&k) { set.insert(k); }`.
6. **Gros handler** que rien de ce qui précède ne débloque → le sortir du
   `rsx!` dans un `let on_x = move |e| { … };`, l'attribut devient
   `onx: on_x` (le corps est alors du Rust que seul `cargo fmt` touche).
7. **Bras de `match` dont le corps est un autre `match` multi-ligne**
   (`A => match y { … },`) → `A => {` / `match y { … }` / `}`.

Un commentaire **à l'intérieur** de l'une de ces constructions est ce qui
rend le bug visible (commentaire déplacé ou dupliqué) ; le commentaire n'est
pas en cause, la construction l'est.

## Pourquoi maintenant

- Le formatage `rsx` devient uniforme sans effort de relecture ;
- les constructions ci-dessus sont aussi celles qui rendent le balisage dur
  à lire (gros blocs d'expression, handlers interminables) : la garde pousse
  vers des composants plus plats ;
- le coût du passage initial croît avec la taille du front.

Version de référence : `dx` 0.7.10 (épinglée en CI). La 0.8.0-alpha.1 a été
testée le 2026-10-02 : mêmes défauts, pas de gain à monter.
