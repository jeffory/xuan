"""Convert a ComfyUI workflow from the editor's save format (``nodes`` and
``links``, the way Comfy publishes its templates) to the API format that
``POST /api/v2/jobs`` runs, using the node definitions from
``/api/object_info``.

It follows ComfyUI_frontend's ``graphToPrompt`` for what Comfy's templates
use: widget values in definition order, dynamic combos (``model`` expanding
to ``model.seed`` and so on), the "control after generate" value saved after
seeds, Reroute and Primitive nodes, and list values wrapped as
``{"__value__": [...]}`` so the server does not read them as links. Anything
else raises ``Unsupported``, so the caller can keep the last version that
converted.
"""

DYNAMIC = "COMFY_DYNAMICCOMBO_V3"
AUTOGROW = "COMFY_AUTOGROW_V3"
WIDGET_TYPES = {"INT", "FLOAT", "STRING", "BOOLEAN", "COMBO"}
CONTROL_VALUES = {"fixed", "increment", "decrement", "randomize"}
SEED_NAMES = {"seed", "noise_seed"}
MUTED, BYPASSED = 2, 4
MISSING = object()
PRIMITIVE = object()


class Unsupported(Exception):
    """The workflow uses something this converter does not handle."""


def to_api(workflow, defs, output, select=None):
    """Return ``(graph, specs)`` for the nodes that node ``output`` depends on.

    ``graph`` is the API-format workflow. ``specs[node_id][input]`` is the
    definition of every input the converted nodes have, sockets included, so
    callers can check the inputs they set and read a combo's options.
    ``select`` maps ``"<class>.<dynamic combo>"`` to an option to use instead
    of the saved one: inputs both options have keep their saved values, the
    others get their defaults."""
    if not isinstance(workflow, dict) or not isinstance(workflow.get("nodes"), list):
        raise Unsupported("not a ComfyUI workflow in the editor's format")
    nodes = {str(node.get("id")): node for node in workflow["nodes"] if isinstance(node, dict)}
    links = _links(workflow)
    subgraphs = {s.get("id") for s in (workflow.get("definitions") or {}).get("subgraphs") or [] if isinstance(s, dict)}
    select = select or {}

    def source(link_id, seen=()):
        """``(node id, slot)`` a link comes from, through Reroutes; PRIMITIVE
        for a Primitive node (the target keeps the value in its own widget)."""
        if link_id not in links:
            raise Unsupported(f"link {link_id} is missing")
        src, slot = links[link_id]
        node = nodes.get(src)
        if node is None:
            raise Unsupported(f"link {link_id} comes from missing node {src}")
        kind = node.get("type")
        if kind == "PrimitiveNode":
            return PRIMITIVE
        if kind == "Reroute":
            if src in seen:
                raise Unsupported("Reroute nodes form a loop")
            upstream = next((i.get("link") for i in node.get("inputs") or [] if i.get("link") is not None), None)
            return None if upstream is None else source(upstream, seen + (src,))
        return src, slot

    output = str(output)
    graph, specs = {}, {}
    pending = [output]
    while pending:
        node_id = pending.pop()
        if node_id in graph:
            continue
        node = nodes.get(node_id)
        if node is None:
            raise Unsupported(f"node {node_id} is missing")
        kind = node.get("type")
        if kind in subgraphs:
            raise Unsupported(f"node {node_id} is a subgraph")
        if node.get("mode") == BYPASSED:
            raise Unsupported(f"node {node_id} ({kind}) is bypassed")
        if node.get("mode") == MUTED:
            raise Unsupported(f"node {node_id} ({kind}) is muted")
        definition = defs.get(kind)
        if not isinstance(definition, dict):
            raise Unsupported(f"node {node_id}: Comfy has no node called {kind}")
        inputs, node_specs = _convert_node(node_id, node, definition, source, select)
        title = node.get("title") or definition.get("display_name") or kind
        graph[node_id] = {"class_type": kind, "inputs": inputs, "_meta": {"title": title}}
        specs[node_id] = node_specs
        for value in inputs.values():
            if _is_link(value):
                pending.append(value[0])
    return graph, specs


def _links(workflow):
    links = {}
    for link in workflow.get("links") or []:
        if isinstance(link, dict):
            links[link.get("id")] = (str(link.get("origin_id")), link.get("origin_slot", 0))
        elif isinstance(link, list) and len(link) >= 3:
            links[link[0]] = (str(link[1]), link[2])
    return links


def _is_link(value):
    return isinstance(value, list) and len(value) == 2 and isinstance(value[0], str) and isinstance(value[1], int)


def _entries(section, order=None):
    """``(name, spec)`` of a definition's inputs: required, then optional."""
    section = section or {}
    for group in ("required", "optional"):
        items = section.get(group) or {}
        names = (order or {}).get(group) or list(items)
        for name in names:
            if name in items:
                yield name, items[name]


def _parts(spec):
    if not isinstance(spec, (list, tuple)) or not spec:
        return None, {}
    opts = spec[1] if len(spec) > 1 and isinstance(spec[1], dict) else {}
    return spec[0], opts


def _is_widget(kind, opts):
    if opts.get("forceInput"):
        return False
    if isinstance(kind, list) or kind in WIDGET_TYPES or kind == DYNAMIC:
        return True
    return "default" in opts or bool(opts.get("socketless"))


def _options(kind, opts):
    if isinstance(kind, list):
        return kind
    options = opts.get("options")
    return options if isinstance(options, list) else None


def default_value(kind, opts):
    """What the editor puts in a widget that has no saved value."""
    if "default" in opts:
        return opts["default"]
    if kind == DYNAMIC:
        options = opts.get("options") or []
        if options and isinstance(options[0], dict):
            return options[0].get("key")
    options = _options(kind, opts)
    if options:
        return options[0]
    if kind in ("INT", "FLOAT"):
        return opts.get("min", 0)
    if kind == "BOOLEAN":
        return False
    if kind == "STRING":
        return ""
    return MISSING


def _check(where, kind, opts, value):
    """Raise Unsupported when ``value`` cannot be this widget's: the sign of
    widget values that no longer line up with the definition."""
    def bad(expected):
        raise Unsupported(f"{where}: expected {expected}, found {value!r}")

    if kind == "INT":
        if isinstance(value, bool) or not (isinstance(value, int) or (isinstance(value, float) and value.is_integer())):
            bad("a whole number")
    elif kind == "FLOAT":
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            bad("a number")
    elif kind == "BOOLEAN":
        if not isinstance(value, bool):
            bad("true or false")
    elif kind == "STRING":
        if not isinstance(value, str):
            bad("text")
    elif kind == DYNAMIC:
        if not isinstance(value, str):
            bad("an option name")
    elif kind == "COMBO" or isinstance(kind, list):
        options = _options(kind, opts)
        if opts.get("multiselect") or opts.get("image_upload"):
            return
        if options is not None and value not in options:
            bad(f"one of {len(options)} options")


def _choice(where, opts, key):
    for option in opts.get("options") or []:
        if isinstance(option, dict) and option.get("key") == key:
            return option
    raise Unsupported(f"{where}: no option {key!r}")


def _convert_node(node_id, node, definition, source, select):
    kind = node.get("type")
    sockets = {i.get("name"): i for i in node.get("inputs") or [] if isinstance(i, dict)}
    saved = node.get("widgets_values")
    by_name = saved if isinstance(saved, dict) else None
    queue = saved if isinstance(saved, list) else []
    position = [0]
    inputs, specs = {}, {}

    def take():
        if position[0] < len(queue):
            position[0] += 1
            return queue[position[0] - 1]
        return MISSING

    def link(name, socket):
        found = source(socket["link"])
        if found is PRIMITIVE:
            return False
        if found is not None:
            inputs[name] = [found[0], found[1]]
        return True

    def defaults(prefix, entries):
        """Inputs of a newly selected dynamic option that the saved one lacked."""
        for name, spec in entries:
            full = prefix + name
            sub_kind, opts = _parts(spec)
            specs[full] = spec
            if full in inputs or not _is_widget(sub_kind, opts) or sub_kind == AUTOGROW:
                continue
            value = default_value(sub_kind, opts)
            if value is MISSING:
                raise Unsupported(f"node {node_id} ({kind}): {full} has no default")
            inputs[full] = _wrap(value)
            if sub_kind == DYNAMIC:
                defaults(full + ".", _entries(_choice(f"node {node_id}", opts, value).get("inputs")))

    def walk(prefix, entries):
        for name, spec in entries:
            full = prefix + name
            sub_kind, opts = _parts(spec)
            specs[full] = spec
            socket = sockets.get(full)
            if sub_kind == AUTOGROW:
                # Slots are sockets named "<input>.<slot>"; only linked ones count.
                template = ((opts.get("template") or {}).get("input") or {}).get("required") or {}
                slot_spec = next(iter(template.values()), ["*"])
                for socket_name, slot in sockets.items():
                    if socket_name and socket_name.startswith(full + "."):
                        specs[socket_name] = slot_spec
                        if slot.get("link") is not None:
                            link(socket_name, slot)
                continue
            widget = _is_widget(sub_kind, opts) and not (socket is not None and "widget" not in socket)
            if not widget:
                if socket is not None and socket.get("link") is not None:
                    link(full, socket)
                continue
            value = by_name.get(full, MISSING) if by_name is not None else take()
            if value is MISSING:
                value = default_value(sub_kind, opts)
                if value is MISSING:
                    raise Unsupported(f"node {node_id} ({kind}): no value for {full}")
            where = f"node {node_id} ({kind}) {full}"
            _check(where, sub_kind, opts, value)
            linked = socket is not None and socket.get("link") is not None and link(full, socket)
            if sub_kind == DYNAMIC:
                chosen = _choice(where, opts, value)
                inputs[full] = value
                walk(full + ".", _entries(chosen.get("inputs")))
                override = select.get(f"{kind}.{full}")
                if override is not None and override != value:
                    replacement = _choice(where, opts, override)
                    keep = {full + "." + n for n, _ in _flatten(replacement.get("inputs"))}
                    for key in [k for k in inputs if k.startswith(full + ".") and k not in keep]:
                        del inputs[key]
                        specs.pop(key, None)
                    inputs[full] = override
                    defaults(full + ".", _entries(replacement.get("inputs")))
                continue
            if not linked:
                if isinstance(value, float) and sub_kind == "INT":
                    value = int(value)
                inputs[full] = _wrap(value)
            if by_name is None and (opts.get("control_after_generate") or (sub_kind == "INT" and name in SEED_NAMES)):
                if position[0] < len(queue) and queue[position[0]] in CONTROL_VALUES:
                    position[0] += 1

    walk("", _entries(definition.get("input"), definition.get("input_order")))
    return inputs, specs


def _flatten(section, prefix=""):
    """Every input name of an option, nested dynamic options included."""
    for name, spec in _entries(section):
        yield prefix + name, spec
        kind, opts = _parts(spec)
        if kind == DYNAMIC:
            for option in opts.get("options") or []:
                if isinstance(option, dict):
                    yield from _flatten(option.get("inputs"), prefix + name + ".")


def _wrap(value):
    return {"__value__": value} if isinstance(value, list) else value


def upstream(graph, output):
    """``graph`` reduced to node ``output`` and the nodes it depends on."""
    keep, pending = set(), [str(output)]
    while pending:
        node_id = pending.pop()
        if node_id in keep or node_id not in graph:
            continue
        keep.add(node_id)
        for value in graph[node_id].get("inputs", {}).values():
            if _is_link(value):
                pending.append(value[0])
    return {node_id: node for node_id, node in graph.items() if node_id in keep}
