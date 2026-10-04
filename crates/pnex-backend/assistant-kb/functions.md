---
id: functions
title: Functions
kind: feature
pages: /functions
nodes: pnex_function
err_codes: functions-write-forbidden, functions-runtime-disabled
tools: 
tags: function, functions, javascript, js, starlark, python, code, script, quickjs, fonction, code utilisateur
---
The Functions page (Automation menu) is the organization's registry of versioned user functions, written in JavaScript (QuickJS) or Starlark (Python-like, deterministic). A function becomes a Function node in flows when a built-in node is not enough.

## What you can do

- **New function**: name, language (Starlark for computations, JavaScript for JSON and string handling), a starting template and a description.
- Declare the node's ports at the top of the code with directives, e.g. `@input value number "Measured value"` and `@output alarm bool "..."`. Each input becomes an input row of the flow node, each output an output port. The editor flags names used in code but not declared (**Declare all** / **Add all**).
- **Save** appends a version; **History** lists versions and can load an older one. **Format** and **Insert a snippet** help writing; the **Reference** panel lists the functions available in each language.
- **Test run** executes the code currently in the editor (no save needed) with input values or an incoming msg, and shows the console, the duration and the output messages per port.
- In the flow editor, add a **Function** node and pick the function and version.

## Good to know

- The return value replaces the message payload; it is not merged into it.
- A flow pins the function version chosen in its node: editing a function changes nothing in deployed flows until they are redeployed.
- A function used by a deployed flow cannot be deleted.
- If someone saved a newer version first, the editor reloads.
- Testing needs the flow runtime enabled on the server.
- The assistant cannot pick a function inside a flow node: leave the node for you to select the function in the editor.
- Viewers see functions read-only.
