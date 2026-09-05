"""
İmbik AoE (The Art of Electronics) Geometric Schematic Engine
Converts evolved analog circuits into textbook publication-quality schematics
following strict Horowitz & Hill conventions:

1. Signal flow strictly Left-to-Right (Input on left, Output on right).
2. Top-to-Bottom power hierarchy: a VCC bus across the top, a VEE bus across the
   bottom, GND symbols in the lower half.
3. 100% Orthogonal routing: only horizontal tracks and vertical trunks.
4. Transistors are first-class devices on the signal path: base fed from the left,
   collector toward the positive rail, emitter toward the negative rail (mirrored
   for PNP), matching how schemdraw draws them.
5. Every component that touches a supply rail is drawn as a vertical stub to the
   corresponding bus - it is never dropped.
6. Upper half (Y > 0) houses op-amp feedback loops in a nested multi-tier ladder;
   lower half (Y < 0) houses ground shunts and input bypass loops.

COVERAGE GUARANTEE
------------------
Every component in the netlist is drawn exactly once. Anything the placement engine
cannot route is emitted into a clearly marked "UNPLACED" strip with net labels, plus a
loud warning on stdout, rather than being silently omitted. A schematic that quietly
drops parts is worse than no schematic, because a human builds hardware from it.
"""

import sys
import json
import heapq
from pathlib import Path
from collections import defaultdict

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8')

try:
    import schemdraw
    import schemdraw.elements as elm
except ImportError:
    print("Error: schemdraw is required. Run with: uv run --with schemdraw python scripts/schematic.py ...")
    sys.exit(1)


NODE_GND, NODE_IN, NODE_OUT, NODE_VCC, NODE_VEE = 0, 1, 2, 3, 4
RAIL_NODES = (NODE_VCC, NODE_VEE)

# Dijkstra edge weights for finding the input -> output signal path.
# Resistors are the cheapest hop, diodes are heavily penalised (they are almost always
# clippers hanging off the path rather than the path itself), and a transistor traversed
# backwards (emitter -> base) is penalised because signal does not flow that way.
W_RESISTOR, W_CAP, W_DIODE = 1.0, 2.0, 50.0
W_BJT_TO_EMITTER, W_BJT_TO_COLLECTOR, W_BJT_REVERSE = 1.0, 1.2, 30.0

# Layout constants
STUB_DX = 3.0     # horizontal pitch between same-direction vertical stubs on a node.
                  # Must exceed the widest stub label (~1.5 units) plus its gap,
                  # otherwise a label runs across the neighbouring stub's wire.
SYM_LEN = 2.0     # drawn length of a series symbol, anchored at its left-hand node

# How far a transistor's collector/emitter sits above/below its base row, measured from
# schemdraw rather than hard-coded, so a symbol change cannot silently misalign wiring.
_BJT_PROBE = elm.BjtNpn()
BJT_PIN_DY = abs(float(_BJT_PROBE.anchors['collector'][1]))
del _BJT_PROBE


def node_label(n):
    """Human-readable net name for a node number."""
    return {NODE_GND: 'GND', NODE_IN: 'IN', NODE_OUT: 'OUT',
            NODE_VCC: 'VCC', NODE_VEE: 'VEE'}.get(n, 'n%d' % n)


def get_element(comp_type, value):
    """Map a two-terminal component type to a SchemDraw element CLASS plus its label.

    A class, not an instance, on purpose. Inside a `with schemdraw.Drawing()` block
    schemdraw implicitly adds every element at construction time; if any other element
    is constructed between that implicit add and an explicit `d += el`, the element is
    emitted twice. Returning the class forces construction and add to happen in the
    same expression, so every component is drawn exactly once.
    """
    val_str = str(value)
    if comp_type == 'C':
        return elm.Capacitor, val_str
    if comp_type == 'D':
        return elm.Diode, val_str
    return elm.Resistor, val_str


def is_pnp(value):
    v = str(value).upper()
    return '3906' in v or 'PNP' in v


def parse_netlist_file(net_path):
    """Parse a SPICE netlist file into a circuit dictionary."""
    vcc, vee, comps = 9.0, -9.0, []
    in_control = False
    with open(net_path, 'r', encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            low = line.lower()
            # The .control ... .endc block holds simulator commands, not devices. Without
            # skipping it, `quit` parsed as a transistor named Q with no nodes.
            if low.startswith('.control'):
                in_control = True
                continue
            if low.startswith('.endc'):
                in_control = False
                continue
            if in_control:
                continue
            if not line or line.startswith('*') or line.startswith('.') or line.startswith('C_stray'):
                continue
            if line.startswith('V_cc'):
                vcc = float(line.split()[4])
                continue
            if line.startswith('V_ee'):
                vee = float(line.split()[4])
                continue
            if line.startswith('V_'):
                continue
            parts = line.split()
            name = parts[0]
            ctype = name[0].upper()
            if not name[1:].isdigit():
                # Not a reference designator (RN/CN/QN/XN) - simulator noise, skip it.
                continue
            cid = int(name[1:])
            need = {'R': 4, 'C': 4, 'D': 4, 'Q': 5, 'X': 7}.get(ctype)
            if need is not None and len(parts) < need:
                print("[WARN] malformed netlist line skipped: %s" % line)
                continue
            if ctype in ('R', 'C', 'D'):
                comps.append({'comp_type': ctype, 'id': cid,
                              'nodes': [int(parts[1]), int(parts[2])], 'value': parts[3]})
            elif ctype == 'Q':
                comps.append({'comp_type': 'Q', 'id': cid,
                              'nodes': [int(p) for p in parts[1:4]],
                              'value': parts[4] if len(parts) > 4 else '2N3904'})
            elif ctype == 'X':
                comps.append({'comp_type': 'X', 'id': cid,
                              'nodes': [int(p) for p in parts[1:6]],
                              'value': parts[6] if len(parts) > 6 else 'TL072'})
    return {'vcc': vcc, 'vee': vee, 'components': comps}


def find_signal_path(twoterms, bjts, target_node):
    """Least-cost IN -> target route through two-terminals AND transistors."""
    adj = defaultdict(list)
    for gi, c in twoterms:
        u, v = c['nodes'][0], c['nodes'][1]
        w = {'R': W_RESISTOR, 'C': W_CAP}.get(c['comp_type'], W_DIODE)
        adj[u].append((v, w))
        adj[v].append((u, w))
    for gi, c in bjts:
        col, base, emi = c['nodes'][0], c['nodes'][1], c['nodes'][2]
        adj[base].append((emi, W_BJT_TO_EMITTER))
        adj[base].append((col, W_BJT_TO_COLLECTOR))
        adj[emi].append((base, W_BJT_REVERSE))
        adj[col].append((base, W_BJT_REVERSE))

    pq = [(0.0, NODE_IN, [NODE_IN])]
    best = {NODE_IN: 0.0}
    while pq:
        cost, curr, path = heapq.heappop(pq)
        if curr == target_node:
            return path
        if cost > best.get(curr, float('inf')):
            continue
        for nxt, w in adj[curr]:
            # Rails and ground are terminals, never intermediate hops on a signal path.
            if nxt in (NODE_GND, NODE_VCC, NODE_VEE) and nxt != target_node:
                continue
            nc = cost + w
            if nc < best.get(nxt, float('inf')):
                best[nxt] = nc
                heapq.heappush(pq, (nc, nxt, path + [nxt]))
    return [NODE_IN]


def draw_bjt(d, c, bx, by, node_x, ny, y_vcc, y_vee):
    # Draw one transistor: symbol, C/B/E pin identity labels, part label, and the routing
    # of its collector and emitter.
    #
    # `by` is the row its BASE sits on - the incoming signal axis. Pin labels matter for
    # breadboard work: a TO-92 has no markings, so the schematic is the only thing telling
    # you which leg is which. A signal pin is routed only when the target column lies to
    # the RIGHT of the device; routing leftwards would drag the wire back across the
    # transistor body, so those fall back to a net label.
    pnp = is_pnp(c['value'])
    col, base, emi = c['nodes']
    q = (elm.BjtPnp if pnp else elm.BjtNpn)().right().at((bx, by))
    d.add(q)

    ba, ca, ea = q.absanchors['base'], q.absanchors['collector'], q.absanchors['emitter']
    d += elm.Label().at((float(ca[0]) + 0.30, float(ca[1]) + 0.34)).label(
        'C', fontsize=8, halign='left')
    d += elm.Label().at((float(ba[0]) - 0.14, by + 0.36)).label(
        'B', fontsize=8, halign='right')
    d += elm.Label().at((float(ea[0]) + 0.30, float(ea[1]) - 0.34)).label(
        'E', fontsize=8, halign='left')
    d += elm.Label().at((bx + 2.3, by + (1.55 if not pnp else -1.55))).label(
        "Q%s\n%s" % (c['id'], c['value']))
    d.add(elm.Dot().at((bx, by)))

    for pin_node, anchor in ((col, ca), (emi, ea)):
        ax, ay = float(anchor[0]), float(anchor[1])
        if pin_node == NODE_VCC:
            d += elm.Line().at((ax, ay)).to((ax, y_vcc))
            d.add(elm.Dot().at((ax, y_vcc)))
        elif pin_node == NODE_VEE:
            d += elm.Line().at((ax, ay)).to((ax, y_vee))
            d.add(elm.Dot().at((ax, y_vee)))
        elif pin_node == NODE_GND:
            d += elm.Line().at((ax, ay)).to((ax, ay - 1.2))
            d += elm.Ground().at((ax, ay - 1.2))
        elif pin_node in node_x and node_x[pin_node] > ax + 0.3:
            tx, ty = node_x[pin_node], ny(pin_node)
            d += elm.Line().at((ax, ay)).to((tx, ay))
            # For the pin that carries the signal onward, ty == ay by construction, so no
            # jog is drawn and the run stays level with the node it feeds.
            if abs(ty - ay) > 1e-9:
                d += elm.Line().at((tx, ay)).to((tx, ty))
            d.add(elm.Dot().at((tx, ty)))
        else:
            d += elm.Line().right().at((ax, ay)).length(0.7).label(
                node_label(pin_node), loc='right')
    return q


def render_circuit_schematic(circuit_dict, outfile, title="Imbik Evolved Circuit"):
    """Render an evolved analog circuit with a full-coverage AoE placement engine."""
    vcc = circuit_dict.get('vcc', 9.0)
    vee = circuit_dict.get('vee', -9.0)
    components = circuit_dict.get('components', [])

    # Global index -> component, so coverage can be tracked across every category.
    twoterms = [(i, c) for i, c in enumerate(components) if c.get('comp_type') in ('R', 'C', 'D')]
    bjts = [(i, c) for i, c in enumerate(components) if c.get('comp_type') == 'Q']
    opamps = [(i, c) for i, c in enumerate(components) if c.get('comp_type') == 'X']
    placed = set()

    has_opamp = len(opamps) > 0
    op_in_plus = op_in_minus = op_out = NODE_OUT
    if has_opamp:
        op_nodes = opamps[0][1].get('nodes', [1, 2, 3, 4, 2])
        op_in_plus, op_in_minus, op_out = op_nodes[0], op_nodes[1], op_nodes[4]

    target_node = op_in_plus if (has_opamp and op_in_plus != NODE_GND) else NODE_OUT
    forward_path = find_signal_path(twoterms, bjts, target_node)
    if forward_path[-1] != target_node and target_node != NODE_IN:
        forward_path.append(target_node)

    # ---- Resolve which device sits on each hop of the signal path ----------------
    def find_hop(u, v):
        for gi, c in twoterms:
            if gi in placed:
                continue
            ns = c['nodes']
            if (ns[0] == u and ns[1] == v) or (ns[0] == v and ns[1] == u):
                return ('2T', gi, c)
        for gi, c in bjts:
            if gi in placed:
                continue
            col, base, emi = c['nodes']
            if u == base and v in (emi, col):
                return ('Q', gi, c)
        return (None, None, None)

    hops = []
    for i in range(len(forward_path) - 1):
        kind, gi, c = find_hop(forward_path[i], forward_path[i + 1])
        if gi is not None:
            placed.add(gi)
        hops.append((forward_path[i], forward_path[i + 1], kind, gi, c))

    # ---- Signal axis height per node ----------------------------------------------
    # A follower's output leaves the transistor at the EMITTER, which sits below the base
    # row (above it, for PNP). Drawing everything downstream back at y=0 left the emitter
    # run and the output terminal on two different rows joined by a visible jog. The axis
    # genuinely steps at every transistor, so track it per node and keep the chain on it.
    node_y = {NODE_IN: 0.0}
    _axis = 0.0
    for _u, _v, _kind, _gi, _c in hops:
        node_y.setdefault(_u, _axis)
        if _kind == 'Q':
            _col, _base, _emi = _c['nodes']
            _step = BJT_PIN_DY if is_pnp(_c['value']) else -BJT_PIN_DY   # emitter side
            if _v == _col:
                _step = -_step
            _axis += _step
        node_y[_v] = _axis

    def ny(node):
        return node_y.get(node, 0.0)

    # ---- Pre-scan vertical attachments so the layout can reserve room for them -----
    # Rail stubs and ground shunts hang vertically off a node. When a node needs more
    # than one in the SAME direction they must spread sideways, and without reserved
    # room they land on top of the incoming series element's symbol.
    pre_up, pre_down = defaultdict(int), defaultdict(int)
    for _gi, _c in twoterms:
        _u, _v = _c['nodes'][0], _c['nodes'][1]
        if _u in RAIL_NODES and _v in RAIL_NODES:
            continue
        if _u in RAIL_NODES or _v in RAIL_NODES:
            _rail = _u if _u in RAIL_NODES else _v
            _other = _v if _u in RAIL_NODES else _u
            (pre_up if _rail == NODE_VCC else pre_down)[_other] += 1
        elif _u == NODE_GND or _v == NODE_GND:
            pre_down[_u if _v == NODE_GND else _v] += 1

    # Extra horizontal space a node needs on its left for stub fan-out.
    def stub_room(node):
        return max(0, max(pre_up[node], pre_down[node]) - 1) * STUB_DX

    # ---- X coordinates -----------------------------------------------------------
    DX, DX_BJT = 3.0, 4.5
    node_x, x = {}, 0.0
    for i, n in enumerate(forward_path):
        if i > 0:
            x += stub_room(n)
        node_x[n] = x
        x += DX_BJT if (i < len(hops) and hops[i][2] == 'Q') else DX
    current_x = x

    op_x = None
    if has_opamp:
        op_x = current_x
        op_out_x = op_x + 2.165 + 1.2
        node_x[op_out] = op_out_x
        node_x[NODE_OUT] = op_out_x
        node_y[op_out] = ny(op_in_plus)
        node_y[NODE_OUT] = ny(op_in_plus)
        current_x = op_out_x + DX
    elif NODE_OUT not in node_x:
        node_x[NODE_OUT] = current_x
        current_x += DX

    for _gi, c in twoterms + bjts:
        for n in c['nodes']:
            if n not in node_x and n not in (NODE_GND, NODE_VCC, NODE_VEE):
                current_x += stub_room(n)
                node_x[n] = current_x
                current_x += DX

    # ---- Classify the remaining two-terminals ------------------------------------
    rail_comps, shunt_comps, input_bypass_comps, bridge_comps = [], [], [], []

    for gi, c in twoterms:
        if gi in placed:
            continue
        u, v = c['nodes'][0], c['nodes'][1]
        u_rail, v_rail = u in RAIL_NODES, v in RAIL_NODES
        if u_rail or v_rail:
            # Touches a supply rail. These used to match no category at all and vanish
            # from the drawing - the single biggest source of missing parts in discrete
            # topologies, where the entire bias network hangs off VCC/VEE.
            rail = u if u_rail else v
            other = v if u_rail else u
            rail_comps.append((gi, c, rail, other, u_rail and v_rail))
            placed.add(gi)
        elif u == NODE_GND or v == NODE_GND:
            shunt_comps.append((gi, c, u if v == NODE_GND else v))
            placed.add(gi)

    output_nodes = {op_out, NODE_OUT}
    if has_opamp and op_in_minus == op_out:
        output_nodes.add(op_in_minus)

    for gi, c in twoterms:
        if gi in placed:
            continue
        u, v = c['nodes'][0], c['nodes'][1]
        if u in forward_path and v in forward_path and u not in output_nodes and v not in output_nodes:
            input_bypass_comps.append((gi, c, u, v))
            placed.add(gi)

    for gi, c in twoterms:
        if gi in placed:
            continue
        bridge_comps.append((gi, c, c['nodes'][0], c['nodes'][1]))
        placed.add(gi)

    # ---- Off-path transistors get their own column further right ------------------
    offpath_bjts = []
    for gi, c in bjts:
        if gi in placed:
            continue
        if c['nodes'][1] not in node_x:
            node_x[c['nodes'][1]] = current_x
        current_x += DX_BJT
        offpath_bjts.append((gi, c))
        placed.add(gi)

    if has_opamp:
        placed.add(opamps[0][0])

    # ---- Vertical extents, then rail bus heights ---------------------------------
    bridge_comps.sort(key=lambda b: abs(node_x.get(b[2], 0) - node_x.get(b[3], 0)))
    track_y, curr_y = {}, 3.0
    for b in bridge_comps:
        track_y[b[0]] = curr_y
        curr_y += 1.4

    input_bypass_comps.sort(key=lambda b: abs(node_x.get(b[2], 0) - node_x.get(b[3], 0)))
    bypass_y, curr_bypass_y = {}, -1.8
    for gi, c, u, v in input_bypass_comps:
        x1, x2 = sorted((node_x.get(u, 0.0), node_x.get(v, 0.0)))
        blocked = any(x1 < node_x.get(s[2], -1e9) < x2 for s in shunt_comps)
        by = curr_bypass_y - (1.0 if blocked else 0.0)
        bypass_y[gi] = by
        curr_bypass_y = by - 1.0

    y_vcc = max(curr_y, 3.0) + 1.6
    y_vee = min(curr_bypass_y, -3.6) - 1.6

    # Vertical attachments are grouped BY DIRECTION. An "up" stub to VCC and a "down"
    # stub to VEE on the same node cannot collide, so both sit exactly on the node's
    # column - which is how a bias divider is drawn in every textbook. Only same-direction
    # stubs need spreading, and on a node that anchors a device (a BJT base) they spread
    # strictly LEFT: the device body occupies the space to the right, and a stub placed
    # there routed its connector wire straight through the transistor.
    device_anchor = set()
    for _u, _v, _kind, _gi, _c in hops:
        if _kind == 'Q':
            device_anchor.add(_c['nodes'][1])
    for _gi, _c in offpath_bjts:
        device_anchor.add(_c['nodes'][1])

    up_count, down_count = defaultdict(int), defaultdict(int)
    for _gi, _c, rail, other, both in rail_comps:
        if not both:
            (up_count if rail == NODE_VCC else down_count)[other] += 1
    for _gi, _c, u in shunt_comps:
        down_count[u] += 1
    up_seen, down_seen = defaultdict(int), defaultdict(int)

    # Column for one vertical stub, plus its index within that node's fan-out.
    #
    # Fan-out always grows LEFTWARDS into the room reserved by `stub_room`. Growing
    # rightwards would cross a device body (a BJT sits immediately right of its base
    # node); growing symmetrically would cross the incoming series element.
    def attach_x(node, direction):
        base = node_x.get(node, 0.0)
        seen = up_seen if direction == 'up' else down_seen
        k = seen[node]
        seen[node] += 1
        return base - k * STUB_DX, k

    # Label sits left of its own stub, aligned so the text grows away from the symbol,
    # and stepped in height so neighbouring stubs' labels never collide.
    def stub_label_anchor(sx, y_rail, k):
        return (sx - 0.5, y_rail * (0.58 - 0.17 * k), 'right')

    nodes_with_shunts = set(s[2] for s in shunt_comps)
    node_in_x = {n: node_x[n] for n in forward_path}
    for gi, c, u, v in input_bypass_comps:
        right_n = v if node_x.get(v, 0) > node_x.get(u, 0) else u
        if right_n in nodes_with_shunts:
            node_in_x[right_n] = node_x[right_n] - 0.8

    x_min = min([0.0] + list(node_x.values())) - 1.5
    x_max = max([0.0] + list(node_x.values())) + 3.0

    unplaced = [(i, c) for i, c in enumerate(components) if i not in placed]

    # =============================== DRAW =========================================
    outfile = Path(outfile)
    outfile.parent.mkdir(parents=True, exist_ok=True)

    with schemdraw.Drawing(file=str(outfile), show=False) as d:
        d.config(unit=2.5, fontsize=10, inches_per_unit=0.45)

        # A. Supply buses
        d += elm.Line().at((x_min, y_vcc)).to((x_max, y_vcc))
        d += elm.Label().at((x_min, y_vcc + 0.5)).label('+%.1fV (VCC)' % vcc)
        d += elm.Line().at((x_min, y_vee)).to((x_max, y_vee))
        d += elm.Label().at((x_min, y_vee - 0.5)).label('%.1fV (VEE)' % vee)

        # B. Input terminal
        x_in, y_in = node_x.get(NODE_IN, 0.0), ny(NODE_IN)
        d += elm.Line().right().at((x_in - 0.8, y_in)).to((x_in, y_in)).label(
            'IN (Node 1)', loc='left')
        d.add(elm.Dot().at((x_in, y_in)))

        # C. Signal path (two-terminals AND transistors)
        for u, v, kind, gi, c in hops:
            if kind == '2T':
                x_u, x_v = node_x[u], node_in_x.get(v, node_x[v])
                y_ax = ny(u)
                el, val = get_element(c['comp_type'], c['value'])
                # Anchor the symbol to the LEFT node at a fixed length and let plain
                # wire cover the rest. A stretched, centre-drawn symbol wanders into the
                # room reserved on the right for the next node's stub fan-out.
                sym_end = min(x_u + SYM_LEN, x_v)
                d += el().right().at((x_u, y_ax)).to((sym_end, y_ax)).label(
                    "%s%s\n%s" % (c['comp_type'], c['id'], val), loc='bottom')
                if x_v > sym_end + 1e-9:
                    d += elm.Line().right().at((sym_end, y_ax)).to((x_v, y_ax))
                d.add(elm.Dot().at((x_u, y_ax)))
                d.add(elm.Dot().at((x_v, y_ax)))
                if abs(x_v - node_x[v]) > 1e-9:
                    d += elm.Line().right().at((x_v, y_ax)).to((node_x[v], y_ax))
                    d.add(elm.Dot().at((node_x[v], y_ax)))
            elif kind == 'Q':
                _b = c['nodes'][1]
                draw_bjt(d, c, node_x[_b], ny(_b), node_x, ny, y_vcc, y_vee)

        # D. Input bypass loops (lower half)
        for gi, c, u, v in input_bypass_comps:
            left_n, right_n = (u, v) if node_x[u] < node_x[v] else (v, u)
            x_start = node_x[left_n]
            x_end = node_in_x.get(right_n, node_x[right_n])
            y_b = bypass_y[gi]
            if abs(x_end - x_start) < 1e-9:
                continue
            d += elm.Line().down().at((x_start, ny(left_n))).to((x_start, y_b))
            d.add(elm.Dot().at((x_start, ny(left_n))))
            el, val = get_element(c['comp_type'], c['value'])
            lbl = "%s%s\n%s" % (c['comp_type'], c['id'], val)
            if c['comp_type'] == 'D' and c['nodes'][0] != left_n:
                d += el().right().reverse().at((x_start, y_b)).to((x_end, y_b)).label(lbl, loc='bottom')
            else:
                d += el().right().at((x_start, y_b)).to((x_end, y_b)).label(lbl, loc='bottom')
            d += elm.Line().up().at((x_end, y_b)).to((x_end, ny(right_n)))
            d.add(elm.Dot().at((x_end, ny(right_n))))

        # E. Rail-connected components -> vertical stubs to the buses
        for gi, c, rail, other, both_rails in rail_comps:
            el, val = get_element(c['comp_type'], c['value'])
            lbl = "%s%s %s" % (c['comp_type'], c['id'], val)
            if both_rails:
                sx = x_max - 1.2
                d += el().up().at((sx, y_vee)).to((sx, y_vcc)).label(lbl, loc='right')
                d.add(elm.Dot().at((sx, y_vee)))
                d.add(elm.Dot().at((sx, y_vcc)))
                continue
            y_rail = y_vcc if rail == NODE_VCC else y_vee
            sx, k_stub = attach_x(other, 'up' if rail == NODE_VCC else 'down')
            ox, oy = node_x.get(other, sx), ny(other)
            if abs(sx - ox) > 1e-9:
                d += elm.Line().at((min(sx, ox), oy)).to((max(sx, ox), oy))
                d.add(elm.Dot().at((ox, oy)))
            if y_rail > 0:
                d += el().up().at((sx, oy)).to((sx, y_rail))
            else:
                d += el().down().at((sx, oy)).to((sx, y_rail))
            lx, ly, lha = stub_label_anchor(sx, y_rail, k_stub)
            d += elm.Label().at((lx, ly)).label(lbl, halign=lha)
            d.add(elm.Dot().at((sx, oy)))
            d.add(elm.Dot().at((sx, y_rail)))

        # F. Ground shunts
        for gi, c, u in shunt_comps:
            sx, _k = attach_x(u, 'down')
            ox, oy = node_x.get(u, sx), ny(u)
            if abs(sx - ox) > 1e-9:
                d += elm.Line().at((min(sx, ox), oy)).to((max(sx, ox), oy))
                d.add(elm.Dot().at((ox, oy)))
            el, val = get_element(c['comp_type'], c['value'])
            d += el().down().at((sx, oy)).length(1.8).label(
                "%s%s\n%s" % (c['comp_type'], c['id'], val), loc='bottom')
            d += elm.Ground().at((sx, oy - 1.8))
            d.add(elm.Dot().at((sx, oy)))

        # G. Op-amp stage
        if has_opamp:
            x_plus, x_out = node_x[op_in_plus], node_x[op_out]
            y_op = ny(op_in_plus)
            op = elm.Opamp(flip=False).right().at((op_x, y_op))
            d.add(op)
            d += elm.Line().right().at((x_plus, y_op)).to((op_x - 0.5, y_op))
            d += elm.Line().down().to((op_x - 0.5, op.in2[1]))
            d += elm.Line().right().to(op.in2)
            d += elm.Line().up().at(op.vd).to((op.vd[0], y_vcc))
            d.add(elm.Dot().at((op.vd[0], y_vcc)))
            d += elm.Line().down().at(op.vs).to((op.vs[0], y_vee))
            d.add(elm.Dot().at((op.vs[0], y_vee)))
            d += elm.Line().right().at(op.out).to((x_out, y_op))
            d.add(elm.Dot().at((x_out, y_op)))
            if op_in_minus == op_out:
                d += elm.Line().up().at((x_out, y_op)).to((x_out, y_op + 2.0))
                d += elm.Line().left().to((op.in1[0] - 0.5, y_op + 2.0))
                d += elm.Line().down().to((op.in1[0] - 0.5, op.in1[1]))
                d += elm.Line().right().to(op.in1)

        # H. Output terminal
        x_2, y_2 = node_x.get(NODE_OUT, current_x), ny(NODE_OUT)
        d += elm.Line().right().at((x_2, y_2)).length(1.0).label('OUT (Node 2)', loc='right')
        d.add(elm.Dot().at((x_2, y_2)))

        # I. Off-path transistors, wired with net labels instead of long routed traces
        for gi, c in offpath_bjts:
            _b = c['nodes'][1]
            bx, by = node_x.get(_b, current_x), ny(_b)
            d += elm.Line().left().at((bx, by)).length(0.7).label(
                node_label(_b), loc='left')
            draw_bjt(d, c, bx, by, node_x, ny, y_vcc, y_vee)

        # J. Bridge / feedback ladder (upper half)
        for gi, c, u, v in bridge_comps:
            xu, xv = node_x.get(u, 0.0), node_x.get(v, 0.0)
            y = track_y[gi]
            x_start, x_end = min(xu, xv), max(xu, xv)
            if abs(x_end - x_start) < 1e-9:
                continue
            n_start, n_end = (u, v) if xu <= xv else (v, u)
            d += elm.Line().up().at((x_start, ny(n_start))).to((x_start, y))
            d += elm.Line().up().at((x_end, ny(n_end))).to((x_end, y))
            d.add(elm.Dot().at((x_start, y)))
            d.add(elm.Dot().at((x_end, y)))
            el, val = get_element(c['comp_type'], c['value'])
            d += el().right().at((x_start, y)).to((x_end, y)).label(
                "%s%s\n%s" % (c['comp_type'], c['id'], val), loc='top')

        # K. COVERAGE GUARANTEE - never silently drop a component
        if unplaced:
            strip_y = y_vee - 3.0
            d += elm.Label().at((x_min + 2.0, strip_y + 1.2)).label(
                'UNPLACED - verify against netlist')
            ux = x_min
            for gi, c in unplaced:
                ctype = c.get('comp_type')
                nets = '-'.join(node_label(n) for n in c.get('nodes', []))
                if ctype in ('R', 'C', 'D'):
                    el, val = get_element(ctype, c['value'])
                    d += el().right().at((ux, strip_y)).to((ux + 2.0, strip_y)).label(
                        "%s%s %s\n[%s]" % (ctype, c['id'], val, nets), loc='bottom')
                else:
                    d += elm.Label().at((ux + 1.0, strip_y)).label(
                        "%s%s %s\n[%s]" % (ctype, c['id'], c.get('value', ''), nets))
                ux += 3.4

        d += elm.Label().at(((x_min + x_max) / 2.0, y_vcc + 1.4)).label(title)

    drawn = len(components) - len(unplaced)
    if unplaced:
        print("[WARN] %s: %d/%d components placed; %d shown in the UNPLACED strip: %s"
              % (outfile, drawn, len(components), len(unplaced),
                 ', '.join('%s%s' % (c['comp_type'], c['id']) for _i, c in unplaced)))
    else:
        print("[OK] AoE Geometric Schematic generated: %s (%d/%d components placed)"
              % (outfile, drawn, len(components)))
    return str(outfile)


def main():
    if len(sys.argv) < 2:
        print("Usage:")
        print("  python scripts/schematic.py <checkpoint.json> <circuit_id> [output.svg]")
        print("  python scripts/schematic.py <circuit.net> [output.svg]")
        sys.exit(1)

    first_arg = sys.argv[1]

    if first_arg.endswith('.net'):
        net_path = Path(first_arg)
        out_svg = sys.argv[2] if len(sys.argv) > 2 else str(net_path.with_suffix('.svg'))
        render_circuit_schematic(parse_netlist_file(net_path), out_svg, title=net_path.stem)
        return

    if len(sys.argv) < 3:
        print("Error: Circuit ID required when specifying a checkpoint JSON file.")
        sys.exit(1)

    cp_path = Path(first_arg)
    circuit_id = int(sys.argv[2])
    out_svg = sys.argv[3] if len(sys.argv) > 3 else "bench/circuit_%02d/schematic.svg" % circuit_id

    with open(cp_path, 'r', encoding='utf-8') as f:
        data = json.load(f)

    entries = data.get('archive', {}).get('entries', [])
    if circuit_id >= len(entries):
        print("Error: Circuit ID %d out of bounds (total: %d)" % (circuit_id, len(entries)))
        sys.exit(1)

    render_circuit_schematic(entries[circuit_id]['circuit'], out_svg,
                             title="Discovery #%d" % circuit_id)


if __name__ == '__main__':
    main()
