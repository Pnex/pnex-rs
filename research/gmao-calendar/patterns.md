# Patterns retenus et anti-patterns

Formulés en règles métier et en patterns UX, sans référence au code des
projets étudiés. Entre parenthèses : où on l'a observé (fiches).

## Patterns retenus (moteur et modèle)

1. **Plan ≠ occurrence ≠ exécution.** La règle produit des occurrences
   datées ; l'exécution (OT) s'attache à une occurrence. On clôture ou on
   annule une occurrence, **on ne retouche jamais le plan** pour gérer un
   cas ponctuel, et l'historique reste intact (ERPNext).
2. **Occurrences calculées, puis matérialisées sur un horizon
   glissant**, de façon déterministe et idempotente. Rejouer le calcul
   (redémarrage, deux réplicas) ne crée ni doublon ni trou, et rattrape
   les échéances manquées (anti-modèle Atlas ; PNEX a déjà l'upsert
   `time_ranges` par `external_id`).
3. **Exception portée par l'occurrence** : une occurrence déplacée ou
   annulée garde un drapeau, et le recalcul ne l'écrase jamais (S3).
4. **Fixe vs flottant = un choix explicite du plan**, affiché dans
   l'éditeur avec son effet. Fixe : depuis la date prévue. Flottant :
   depuis la dernière réalisation (openMAINT, ERPNext actifs).
5. **Trois dates par intervention : prête au plus tôt, idéale,
   échéance.** La fenêtre de tolérance est explicite ; « en retard »
   signifie que l'échéance est dépassée, et c'est **calculé, jamais
   saisi** (Timefold, ERPNext).
6. **Délai d'alerte avant échéance** (par exemple J-14) : l'occurrence
   passe « à préparer » et l'OT peut naître à ce moment-là (openMAINT,
   Atlas).
7. **Compteur alimenté par la mesure**, pas par la saisie. Le seuil
   porte sur l'**accroissement depuis la dernière réalisation** (500 h
   de marche), et une **date prévisionnelle** s'extrapole de la tendance
   (openMAINT Modbus ; PNEX : séries O2 + agrégat `increase`).
8. **Le correctif naît du préventif** : un contrôle hors tolérance
   ouvre une intervention corrective liée (openMAINT 2.3).
9. **L'intervention peut immobiliser l'équipement** : un drapeau bloque
   les autres usages pendant la fenêtre (Odoo « bloquer le poste »).
10. **Aperçu des prochaines dates dans l'éditeur du plan**, avant
    d'enregistrer (ERPNext « générer », B8 : même code front et back).
11. **Indicateurs de fiabilité calculés** (MTBF, MTTR, prochaine panne
    estimée) depuis l'historique (Odoo).

## Patterns retenus (UX)

12. **Une vue calendrier unique**, avec mois, semaine, liste et
    multi-mois pour le plan annuel. Préventif, correctif et demandes y
    sont distingués par la couleur, et les arrêts y apparaissent en
    **événements de fond** (Atlas, FullCalendar, EventCalendar).
13. **Timeline de ressources pour la charge**. Une ligne par technicien,
    regroupée par équipe ; glisser d'une ligne à l'autre réaffecte ; la
    jauge de charge figure dans l'étiquette de ligne (EventCalendar,
    Atlas « charge »).
14. **File « non planifiés » glissable vers le calendrier** : un OT sans
    date se dépose sur un créneau ou une ressource (Atlas, FullCalendar
    dépôt externe).
15. **Le front propose, le serveur dispose.** Glisser envoie une
    demande ; conflits, contraintes et droits sont vérifiés côté serveur
    (refus 409 à code machine), et l'élément revient à sa place en cas
    de refus.
16. **Gantt réservé aux arrêts techniques**, avec dépendances et
    décalage en cascade ; les jours non ouvrés sont exclus du calcul
    (Frappe, openMAINT).
17. **Déclarer une intervention depuis l'objet sur la carte** (POI) ou
    par QR code sur l'équipement (openMAINT 2.4, Atlas).
18. **« Ma journée » pour le terrain** : en retard, aujourd'hui, en
    cours, avec saisie des mesures dans le parcours (Atlas).

## Anti-patterns

1. **Timers persistés comme état de planification** : rafale au
   redémarrage, échéances ratées jamais rattrapées (Atlas #297).
2. **Une seule occurrence future** générée à la clôture de la
   précédente : aucune visibilité à N mois, ni sur la charge (Odoo).
3. **Fréquences en liste fermée** (hebdo, trimestriel…) au lieu d'une
   règle complète (ERPNext).
4. **Deux modules redondants** pour la même notion (ERPNext client vs
   actifs).
5. **Exigences du plan non transmises à l'OT** (signature, preuve) :
   Atlas #216.
6. **Déclencher un plan avec le droit « voir »** (Atlas #268) : chez
   PNEX, toute génération ou exécution est une écriture gardée (R2).
7. **Configuration lourde avant la première date utile** (openMAINT) :
   chez PNEX, un plan sur un device seul doit marcher en une minute.
8. **Fonctions de planification derrière une licence non libre**
   (FullCalendar premium, mobile openMAINT) : jamais de dépendance qui
   enferme une fonction cœur.
9. **Itérer une règle de récurrence sans borne** : toute énumération est
   bornée en nombre et en horizon (garde-fou `rrule`).
