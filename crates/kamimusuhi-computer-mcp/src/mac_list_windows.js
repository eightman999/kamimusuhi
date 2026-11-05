
var se = Application('System Events');
var out = [];
try {
  se.processes.whose({backgroundOnly: false})().forEach(function (p) {
    var name; try { name = p.name(); } catch (e) { return; }
    var titles = [];
    try { titles = p.windows.name(); } catch (e) {}
    out.push({app: name, windows: titles || []});
  });
} catch (e) { out.push({error: String(e)}); }
JSON.stringify(out)
