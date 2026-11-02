---
id: applications
title: Applications — work screens built on the ontology
kind: feature
pages: /apps, /apps/:app/build, /apps/:app/:screen
nodes:
err_codes: app-not-found, app-version-conflict, app-out-of-scope, dashboard-invalid
tools:
tags: application, app, screen, écran, variable, object set, ensemble d'objets, builder, construire, publish, publier, object table, tableau d'objets, form, formulaire, action button, bouton d'action, sales follow-up, suivi commercial
---
An **application** gathers work screens on the organization's objects: search, read, enter, invoke an action. A dashboard is for watching series; an application is for working on objects.

## Reading one
**Applications** in the menu lists those with a published screen, for every member. An application gives no right: its screens show what your role already allows, narrowed to its scope. *Filter to fill in* means a variable has no value yet: fill the input of the screen or pick a row of a table.

## Declaring and building
An owner or admin declares it on the **Access** page, tab Applications: its key, its **scope** (object types) and the **actions** its screens may invoke, written `type.action`. A builder then opens **Build**: **Screens** (name, order, home, then *Edit the screen* for the canvas), **Variables** (values passed between widgets, for this session only) and **Object sets** (a type, filters, a sort; a filter value starting with a dollar, like `$lang`, is a parameter a variable fills).

For example, on invented types `article` and `author`: a set `by_lang` of `article` filtered on `lang` = `$lang`, an **Input** writing the variable `lang`, an **Object table** reading `by_lang` whose rows write the variable `picked` and open a second screen, where an **Object record** shows `picked`, a **Form** edits it and an **Action button** invokes `article.review`.

**Publish** is what readers see: saving a screen changes nothing for them. Publishing refuses a screen naming what the application no longer has (**dashboard-invalid**). Two builders at once: the second is told to reload (**app-version-conflict**).

## Writes
A **Form** and an **Action button** use the same checks as the object page. Outside the scope or the action list the server refuses (**app-out-of-scope**). An action only reaches a deployed flow: nothing physical happens from a screen.

## Shipped applications
A pack can ship one, ready to use: **Sales follow-up** comes with the pack of the same name (stages, leads, activities to do, quotes and invoices). It is then the organization's own: build on it freely; a pack upgrade leaves it as it is.
