---
id: ontology
title: Ontology — objects, links and their history
kind: feature
pages: /ontology, /ontology/object/:id, /ontology/types/:type_key
nodes: action_trigger
err_codes: access-denied, ontology-not-found, ontology-already-exists, ontology-version-conflict, ontology-type-in-use, ontology-write-forbidden, ontology-system-read-only, ontology-link-not-allowed, ontology-link-cardinality, ontology-action-no-flow, ontology-action-unavailable, ontology-action-unknown
tools: describe_ontology, query_ontology, get_object, create_object, update_object, open_link, close_link, propose_action, save_object_type
tags: recurrence, récurrence, maintenance plan, plan de maintenance, ontology, ontologie, object, objet, type, link, lien, relation, pump, pompe, machine, site, line, ligne, equipment, équipement, asset, actif, sensor replacement, remplacement capteur, history, historique, as of, graph, graphe, gallery, galerie, 3D model, modèle 3D, glb, pose, skeleton, squelette, reference picture, image de référence, pack, maintenance, yaml, provenance, action, start maintenance, mettre en maintenance, trigger, déclencheur
---
The Ontology page (Knowledge › Ontology) models the real things of the organization — sites, lines, machines, pumps, or any type you define — as objects with typed properties, linked to each other over time. Devices, media, dashboards, tours, POIs, flows and folders are objects too (system types): they keep their own pages, and each of those pages has an **Object page** button.

## What you can do
- **Objects** tab: filter by type, search by title. **New object** asks what to create (the types you can create, e.g. *Person* once the *Investigation* pack is installed; with no type yet it points to Packs), then the title and the type's properties. Click a row to open its object page.
- **New page** (object page): a page from a pack template made for this type, filled with the object, its links and its hierarchy as of today, then linked to it (see the company groups card for the account plan). List values are written with their display names, in your interface language.
- **Object page**: *Properties* (those a pack added to a type it does not own are grouped under the pack's name); *Links* (valid now, **as of** a past date, or the full history; open a link, close a link); *Graph* (the object and its neighbours, plus the cascading impact downstream); *Gallery* (the media that show the object, see below); *Time* (each time-series property drawn over a window, with the sensors that fed it); *Provenance* (who changed what, when, and from where).
- **Schema** tab: object types (system, from a pack, or yours) with their object counts. The **Link types** tab lists the link types (**New link type** in its header). Owners and admins create a type (key, name, icon, minimal role to write its objects, what it may contain and be contained in, properties) and save new versions of it; every version is kept.
- **Imports** tab: sheets, files and APIs turned into objects and links after a preview (see the *Ontology imports* card). A pack with hierarchies adds a **Hierarchy** tab to the object page (a tree computed on links, at a date).
- **Packs** tab: install a ready-made pack (for example *Augmented maintenance*: sites, lines, machines, pumps), upgrade it, export your schema as YAML or import one. An installed pack that declares settings (for example the scores of *Web presence*) shows **Settings** (owners and admins): your values are kept when the pack is upgraded, a value equal to the pack's is left to the pack, and the flows using them take the new values at their next deploy. For a property with a list of values, the type editor sets how each value is shown, in French and in English, and its colour (map, hierarchy, graph); the value itself never changes, and what you set on a pack's type is kept when the pack is upgraded. For a link type's attributes, use **Value display** in the Link types tab. An upgrade also updates the pack's functions and flows you never edited; one you saved yourself keeps your version, and a message after the upgrade names them. The pack's row keeps listing them; **take the pack's version** next to a name replaces yours with the pack's (yours stays in its version history). An updated flow runs at its next deploy.
- A **type dashboard** (Dashboards › edit › select a gauge, chart, value or indicator › *Type dashboard*) is drawn for any object of a type: each widget reads a series property of the object chosen at the top of the dashboard.

## Gallery
Every object except a media has a **Gallery** tab: the pictures, 3D model and voice that show it. **Add from the library** picks an existing media, **Upload a file** adds a new one (pictures, sound, or a 3D model in GLB). A picture is kept as a *Reference picture*, a GLB as the *3D model*, a sound as the *Voice*.
- The 3D model turns with the mouse. If the file has animations, **Animation** plays one. If it has a skeleton: **Show the skeleton**, pick a **Joint** and turn it with the X, Y and Z sliders; **Rest pose** puts every joint back.
- **Save the pose** (give it a name) keeps the joint angles **and** a thumbnail of the view: the thumbnail appears in the gallery as a *Pose*, and its name appears as a button under the sliders to put the model back in that pose.
- "This model has no skeleton": the GLB carries no joints, it can be looked at but not posed. A model whose joints move nothing was exported without skin weights: fix it in the modelling tool and upload it again.
- Removing an item (bin icon) only takes it out of the gallery: the media stays in Media.

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
- Viewers read everything but change nothing; a type can require the admin role to write its objects (*ontology-write-forbidden*). Only owners and admins edit the schema and install packs (*access-denied* otherwise).
- The assistant can read the schema and the objects, create and update objects, open and close links, save types and propose actions; it never runs an action, never archives an object and never deletes anything.
