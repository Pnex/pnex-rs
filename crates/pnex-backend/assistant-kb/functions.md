---
id: functions
title: Functions
kind: feature
pages: /functions
nodes: pnex_function
err_codes: access-denied, functions-runtime-disabled
tools: 
tags: function, functions, javascript, js, starlark, python, code, script, quickjs, fonction, code utilisateur
---
The Functions page (Automation menu) is the organization's registry of versioned user functions, written in JavaScript (QuickJS) or Starlark (Python-like, deterministic). A function becomes a Function node in flows when a built-in node is not enough.

## What you can do

- **New function**: name, language (Starlark for computations, JavaScript for JSON and string handling), a starting template and a description.
- Declare the node's ports in the **Inputs** and **Outputs** panels next to the code (name, type, default, label), never in the code. Each input becomes an input row of the flow node, each output an output port. The editor flags names the code reads or returns that are not declared (**Declare all** / **Add all**).
- **Message mode** (no output declared): the node has one input and one output, and the value `handle` returns is the outgoing message (`None` drops it; a dict with `payload` is merged into the message; any other value becomes `payload`). **Named ports** (at least one output): `handle` returns a dict whose keys are the output names, each value goes out on its port (a missing key keeps the port silent), or a list, one element per port.
- **Save** appends a version; **History** lists versions and can load an older one. **Format** and **Insert a snippet** help writing; the **Reference** panel lists the functions available in each language.
- **Test run** executes the code currently in the editor (no save needed) with input values or an incoming msg, and shows the console, the duration and the output messages per port.
- In the flow editor, add a **Function** node and pick the function and version.

## Good to know

- In message mode, a returned dict without `payload` replaces the payload; a dict with `payload` is merged into the message (to set `title`, `topic` and so on).
- In named-ports mode, a key that is not a declared output goes nowhere: a function that returns `{"payload": …}` with outputs declared sends nothing.
- A flow pins the function version chosen in its node: editing a function changes nothing in deployed flows until they are redeployed.
- A function used by a deployed flow cannot be deleted.
- If someone saved a newer version first, the editor reloads.
- Testing needs the flow runtime enabled on the server.
- The assistant can list, read, check, test (sandbox) and write functions (code and declared ports), each save being a new version; it never changes the version a deployed flow pins — you select it in the node and redeploy.
- Viewers see functions read-only.
