Repair MiniJinja CLI automatic autoescape suffix handling.

In auto mode, inspect the output filename suffix immediately before one template wrapper suffix: .j2, .jinja, or .jinja2. Match wrapper and output suffixes without regard to ASCII letter case. For example, report.html.jinja2 uses HTML escaping and report.JSON.JINJA uses JSON escaping. Keep the current behavior for ordinary .html, .xml, .json, .yaml, and .yml names, and keep explicit html, json, and none modes overriding filename detection.

Update the --autoescape long help and add focused CLI regression coverage.
