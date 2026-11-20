---
id: maintenance-parts
title: Spare parts — parts lists, part references and replacements
kind: feature
pages: /parts, /ontology
nodes:
err_codes:
tools: query_ontology, get_object
tags: spare part, pièce détachée, parts list, nomenclature, part reference, référence de pièce, supersession, remplacement, replaced by, remplacée par, equipment model, modèle d'appareil, maintenance, GMAO, CMMS, serial number, numéro de série, equivalent part, pièce équivalente
---
The **Augmented maintenance** pack keeps the spare parts of the org with their references and the parts lists of each equipment model. Everything stays private to the org.

## What it holds
- An **equipment model** carries its **parts list**: each line says which part, at which position, how many, and for which serial numbers, build dates and variant (for example 230 V). An installed machine, pump, controller or compressor is linked to its model by **is an instance of**, and keeps its own serial number and **Manufactured on** date.
- A **part reference** is a way a manufacturer or a distributor writes a part. Its title keeps the reference as written; the reference itself is stored with letters and digits only, in upper case, so `ab-12.3/x`, `AB 12.3 X` and `AB123X` are the same reference. A part can have many references, including old ones.
- A part that **supersedes** another replaces it. Parts can also be **equivalent** or **compatible** (with conditions).
- The maker of a model or a part is an organization linked by **manufactures** (pack **Maintenance partners**).

## Finding a part
Open **Spare parts** in the **Operate** menu and type a reference, even partial: spaces, dashes, dots and case do not matter, at least 3 letters or digits. Each part found shows its references and, when it was replaced, **Replaced by** with the newest part. Open a row to see the part and its links.

## The parts of an equipment
An equipment linked to its model by **is an instance of** shows **Parts list (model)** on its page: only the lines that fit its serial number, its **Manufactured on** date and its **Variant**; **Show the other lines of the model** shows the rest, greyed. A value the equipment lacks keeps the line. **Add to the work order** puts the part in the equipment's open work order (requested, planned, in progress or on hold), made when there is none. On a work order's page, **Export the parts (CSV)** gives part number, part and quantity.

## Importing parts lists
On the **Imports** tab of the ontology, use **Parts lists (CSV)** with one line per part of a model and these columns: `model`, `model_name`, `position`, `part_number`, `part_name`, `reference`, `quantity`, `serial_from`, `serial_to`, `date_from` (YYYY-MM-DD), `date_to`, `variant`. A manufacturer's file is first saved in these columns. A line without a reference still adds the part to the list. Replacements come from **Part supersessions (CSV)**: `old_part_number`, `new_part_number`, `since`. Each import shows a preview before anything is written.

## Troubleshooting
- **A reference finds nothing**: check that at least 3 letters or digits are typed, then that the parts list holding it was imported and applied (Imports tab, run state *Applied*).
- **A reference is refused** when created by hand: it holds no letter nor digit (only dashes or dots). Type the reference itself.
- **The replacement is missing**: the supersession links the new part to the old one; import it with **Part supersessions (CSV)** or add the link **supersedes** on the new part's page.
