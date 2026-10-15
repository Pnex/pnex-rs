---
id: ontology
title: Ontology — objects, links and their history
kind: feature
pages: /ontology, /ontology/object/:id, /ontology/types/:type_key
nodes: action_trigger
err_codes: ontology-not-found, ontology-already-exists, ontology-version-conflict, ontology-type-in-use, ontology-write-forbidden, ontology-admin-required, ontology-system-read-only, ontology-link-not-allowed, ontology-link-cardinality, ontology-action-no-flow, ontology-action-unavailable, ontology-action-unknown
tools: describe_ontology, query_ontology, get_object, create_object, update_object, open_link, close_link, propose_action, save_object_type
tags: recurrence, récurrence, maintenance plan, plan de maintenance, ontology, ontologie, object, objet, type, link, lien, relation, pump, pompe, machine, site, line, ligne, equipment, équipement, asset, actif, sensor replacement, remplacement capteur, history, historique, as of, graph, graphe, pack, maintenance, yaml, provenance, action, start maintenance, mettre en maintenance, trigger, déclencheur
---
The Ontology page (Knowledge › Ontology) models the real things of the organization — sites, lines, machines, pumps, or any type you define — as objects with typed properties, linked to each other over time. Devices, media, dashboards, tours, POIs, flows and folders are objects too (system types): they keep their own pages, and each of those pages has an **Object page** button.

## What you can do
- **Objects** tab: filter by type, search by title. **New object** asks what to create (the types you can create, e.g. *Person* once the *Investigation* pack is installed; with no type yet it points to Packs), then the title and the type's properties. Click a row to open its object page.
- **Object page**: *Properties*; *Links* (valid now, **as of** a past date, or the full history; open a link, close a link); *Graph* (the object and its neighbours, plus the cascading impact downstream); *Time* (each time-series property drawn over a window, with the sensors that fed it); *Provenance* (who changed what, when, and from where).
- **Schema** tab: object types (system, from a pack, or yours) with their object counts. The **Link types** tab lists the link types (**New link type** in its header). Owners and admins create a type (key, name, icon, minimal role to write its objects, what it may contain and be contained in, properties) and save new versions of it; every version is kept.
- **Packs** tab: install a ready-made pack (for example *Augmented maintenance*: sites, lines, machines, pumps), upgrade it, export your schema as YAML or import one.
- A **type dashboard** (Dashboards › edit › select a gauge, chart, value or indicator › *Type dashboard*) is drawn for any object of a type: each widget reads a series property of the object chosen at the top of the dashboard.

## Property kinds
Text, number (unit, min, max), yes/no, date, date and time, list of values, position, web link, reference to another object, **recurrence** (every N days/weeks/months/years: its occurrences show in the Calendar), **time series** and **events**. A time series holds no value in the object: it is fed by a sensor bound to it.

## Binding and replacing a sensor
On the object page, *Time* › **Bind a sensor**: choose the device and the metric it publishes. When the sensor is replaced, **Replace the sensor**: the old binding is closed and a new one opens from the new device. The object's curve stays continuous across both sensors, and a type dashboard follows the new sensor without any change.

## Actions
A type may declare **actions** (for example *Start maintenance* on a pump, from the Augmented maintenance pack; other types get theirs through the type's YAML, Packs › Export / Import). On the object page, the action buttons sit next to *Edit*: a click opens a form with the action's parameters, then **Run** sends it.
- The platform does nothing by itself: the action reaches the **deployed flows** holding an *Action trigger* node for it (Flows › palette › Triggers), and those flows decide what happens (notify a technician, write a device, record an event). Each run is kept in the object's *Provenance* tab, with its parameters and the flows that received it.
- "No deployed flow handles this action" (*ontology-action-no-flow*): open Flows, add an *Action trigger* node, pick the type and the action, wire what should happen, then deploy the flow.
- Deploying refuses a trigger whose action no longer exists in the type (*ontology-action-unknown*): pick another action in the node.
- *ontology-action-unavailable*: the action bus is down; nothing was sent, try again later.
- The assistant can **propose** an action with its parameters filled in (a *Review and run* button under its answer); only you run it.

## Good to know
- Nothing is overwritten: closing a link keeps it in the history, archiving an object closes it, deleting a device or a media closes its object. *As of* shows the links valid at that date.
- An open link takes a place: a series property is fed by one sensor at a time, and some link types allow one target per source. A second one is refused (*ontology-link-cardinality*): close the first one.
- If someone saved the object or the type while you were editing, saving answers *ontology-version-conflict*: reload and redo your change.
- System objects (devices, media…) are created and deleted from their own pages (*ontology-system-read-only*); you may add your own properties to a system type in the Schema tab.
- Viewers read everything but change nothing; a type can require the admin role to write its objects (*ontology-write-forbidden*). Only owners and admins edit the schema and install packs (*ontology-admin-required*).
- The assistant can read the schema and the objects, create and update objects, open and close links, save types and propose actions; it never runs an action, never archives an object and never deletes anything.
