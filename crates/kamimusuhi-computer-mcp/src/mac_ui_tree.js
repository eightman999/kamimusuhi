
var se = Application('System Events');
var target = __APP__;
var DEPTH = __BOUND__, LIMIT = __LIMIT__, seen = 0;
function pick(el) {
  function get(f) { try { return el[f](); } catch (e) { return undefined; } }
  var pos = {}; try { var p = el.position(); pos = {x: p[0], y: p[1]}; } catch (e) {}
  var size = {}; try { var s = el.size(); size = {w: s[0], h: s[1]}; } catch (e) {}
  return {role: get('role'), name: get('name'), title: get('title'),
          description: get('description'), value: get('value'),
          position: pos, size: size};
}
function walk(el, depth, out) {
  if (seen >= LIMIT || depth < 0) return;
  var node = pick(el); seen++;
  var kids = [];
  if (depth > 0 && seen < LIMIT) {
    try {
      el.uiElements().forEach(function (k) {
        if (seen < LIMIT) kids.push(walk(k, depth - 1, out));
      });
    } catch (e) {}
  }
  if (kids.length) node.children = kids;
  return node;
}
var out = [];
try {
  var proc = se.processes.whose({name: target})();
  if (!proc.length) proc = se.processes.whose({_match: [ObjectSpecifier().name, target]})();
  var p = proc[0];
  p.windows().forEach(function (w) { if (seen < LIMIT) out.push(walk(w, DEPTH, out)); });
} catch (e) { JSON.stringify({error: String(e)}); }
JSON.stringify({app: target, elements: out, truncated: seen >= LIMIT})
