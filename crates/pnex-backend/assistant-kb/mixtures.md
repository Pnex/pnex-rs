---
id: mixtures
title: Fluid Mixtures
kind: feature
pages: /mixtures
nodes: cool_prop
err_codes: fluid-write-forbidden
tools: 
tags: mixture, mixtures, fluid, coolprop, refrigerant, glycol, thermodynamics, diagram, mélange, fluide, thermo
---
The Fluid Mixtures page (Automation menu) manages the organization's custom fluid mixtures, computed with the embedded CoolProp engine. A mixture defined here is reusable in the CoolProp flow node and in the thermodynamic diagrams of dashboards.

## What you can do

- **New mixture**: a name, a description, a **Fraction basis** (Mole or Mass) and its **Components**: **+ Add component**, then a CoolProp fluid name (e.g. Propane) and its fraction (e.g. 0.5).
- **Edit** or **Delete** a mixture from its row; the table shows its components, basis and molar mass.
- Search mixtures by name.
- Use it in a flow: in the **CoolProp** node inspector, the fluid picker lists the organization mixtures next to the CoolProp fluids.
- Use it in a dashboard: the thermodynamic diagram widget (log(p)-h, T-s) offers "Organization mixtures" in its Fluid / mixture picker.

## Good to know

- With the mass basis, fractions are converted to mole fractions with CoolProp molar masses when you save.
- The server checks the mixture with CoolProp on save and shows the error if a fluid or fraction is invalid.
- Owners, admins and members can manage mixtures; viewers see them read-only.
