"""
İmbik AoE (The Art of Electronics) Geometric Schematic Engine
Converts evolved analog circuits into textbook publication-quality schematics
following strict Horowitz & Hill conventions:
1. Signal flow strictly Left-to-Right (Input on left, Output on right).
2. Top-to-Bottom power hierarchy (VCC +9V on top, VEE -9V and GND at bottom).
3. 100% Orthogonal routing: Zero diagonal lines, only horizontal tracks and vertical trunks.
4. Input bypass components (diodes/resistors in parallel across input chain) sit cleanly in the
   lower half (Y < 0) directly below the series resistors, completely separated from Ground shunts.
   - If a node has both an incoming bypass and a Ground shunt, they are staggered horizontally:
     the bypass enters at X_node - 0.8, while the Ground shunt drops at X_node.
   - Zero lines ever pass through capacitor or diode symbols.
5. Lower half (Y < 0) houses vertical Ground shunts (dropping to GND) and input bypass loops.
6. Upper half (Y > 0) houses all op-amp feedback loops in a nested multi-tier ladder (starting at Y = 3.0).
7. Op-amp oriented strictly rightward with canonical follower loop and clear power rail labels.
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


def get_element(comp_type, value):
    """Map component type to SchemDraw element with clean labels."""
    val_str = str(value)
    if comp_type == 'R':
        return elm.Resistor(), f"{val_str}"
    elif comp_type == 'C':
        return elm.Capacitor(), f"{val_str}"
    elif comp_type == 'D':
        return elm.Diode(), f"{val_str}"
    else:
        return elm.Resistor(), f"{val_str}"


def parse_netlist_file(net_path):
    """Parse a SPICE netlist file into a circuit dictionary."""
    vcc = 9.0
    vee = -9.0
    comps = []
    with open(net_path, 'r', encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('*') or line.startswith('.') or line.startswith('C_stray'):
                continue
            if line.startswith('V_cc'):
                parts = line.split()
                vcc = float(parts[4])
                continue
            if line.startswith('V_ee'):
                parts = line.split()
                vee = float(parts[4])
                continue
            if line.startswith('V_'):
                continue
            parts = line.split()
            name = parts[0]
            ctype = name[0].upper()
            cid = int(name[1:]) if name[1:].isdigit() else 1
            if ctype in ['R', 'C', 'D']:
                n1 = int(parts[1])
                n2 = int(parts[2])
                val = parts[3]
                comps.append({'comp_type': ctype, 'id': cid, 'nodes': [n1, n2], 'value': val})
            elif ctype == 'X':
                nodes = [int(p) for p in parts[1:6]]
                val = parts[6] if len(parts) > 6 else 'TL072'
                comps.append({'comp_type': ctype, 'id': cid, 'nodes': nodes, 'value': val})
    return {'vcc': vcc, 'vee': vee, 'components': comps}


def render_circuit_schematic(circuit_dict, outfile, title="İmbik Evolved Circuit"):
    """
    Renders an evolved analog circuit using an AoE topological placement engine.
    """
    vcc = circuit_dict.get('vcc', 9.0)
    vee = circuit_dict.get('vee', -9.0)
    components = circuit_dict.get('components', [])

    opamps = [c for c in components if c.get('comp_type') == 'X']
    twoterms = [c for c in components if c.get('comp_type') in ['R', 'C', 'D']]

    # 1. Build adjacency graph (excluding power rails 3, 4 and GND 0 for forward path)
    adj = defaultdict(list)
    for idx, c in enumerate(twoterms):
        ns = c.get('nodes', [])
        if len(ns) >= 2:
            u, v = ns[0], ns[1]
            adj[u].append((v, idx, c))
            adj[v].append((u, idx, c))

    # Find primary opamp pins
    has_opamp = len(opamps) > 0
    op_in_plus = 1
    op_in_minus = 2
    op_out = 2
    if has_opamp:
        op_nodes = opamps[0].get('nodes', [1, 2, 3, 4, 2])
        op_in_plus = op_nodes[0] if len(op_nodes) > 0 else 1
        op_in_minus = op_nodes[1] if len(op_nodes) > 1 else 2
        op_out = op_nodes[4] if len(op_nodes) > 4 else 2

    # 2. Find forward path from Node 1 to OpAmp non-inverting input (or inverting if inverting amp)
    # Prefer R (weight 1.0), then C (weight 2.0), heavily penalize D (weight 50.0)
    target_node = op_in_plus if (has_opamp and op_in_plus != 0) else (op_in_minus if has_opamp else 2)
    pq = [(0.0, 1, [1])]
    best_cost = {1: 0.0}
    forward_path = [1]

    while pq:
        cost, curr, path = heapq.heappop(pq)
        if curr == target_node:
            forward_path = path
            break
        if cost > best_cost.get(curr, float('inf')):
            continue
        for nxt, idx, c in adj[curr]:
            if nxt in [0, 3, 4]:
                continue
            ctype = c.get('comp_type', 'R')
            edge_w = 1.0 if ctype == 'R' else (2.0 if ctype == 'C' else 50.0)
            new_cost = cost + edge_w
            if new_cost < best_cost.get(nxt, float('inf')):
                best_cost[nxt] = new_cost
                heapq.heappush(pq, (new_cost, nxt, path + [nxt]))

    if forward_path[-1] != target_node and target_node != 1:
        forward_path.append(target_node)

    # 3. Assign X-coordinates to all nodes along the backbone
    node_x = {}
    current_x = 0.0
    dx_step = 3.0

    for n in forward_path:
        node_x[n] = current_x
        current_x += dx_step

    # Place Op-Amp body coordinates
    op_x = current_x
    if has_opamp:
        # Standard SchemDraw Opamp length is 2.165 units
        # Place Node 2 cleanly 1.2 units to the right of op.out
        op_out_x = op_x + 2.165 + 1.2
        node_x[op_out] = op_out_x
        node_x[2] = op_out_x
        current_x = op_out_x + dx_step
    else:
        node_x[2] = current_x

    # Any remaining unplaced nodes
    for c in twoterms:
        for n in c.get('nodes', []):
            if n not in node_x and n not in [0, 3, 4]:
                node_x[n] = current_x
                current_x += dx_step

    # 4. Classify components into:
    # - backbone_comps (consecutive forward nodes)
    # - shunt_comps (one node is 0 Ground)
    # - input_bypass_comps (both nodes on input forward path before opamp)
    # - bridge_comps (feedback loops to opamp output or inter-stage branches)
    placed_indices = set()
    backbone_comps = []
    shunt_comps = []
    input_bypass_comps = []
    bridge_comps = []

    # Identify backbone
    for i in range(len(forward_path) - 1):
        u = forward_path[i]
        v = forward_path[i+1]
        for idx, c in enumerate(twoterms):
            if idx in placed_indices:
                continue
            ns = c.get('nodes', [])
            if len(ns) >= 2:
                if (ns[0] == u and ns[1] == v) or (ns[0] == v and ns[1] == u):
                    backbone_comps.append((idx, c, u, v))
                    placed_indices.add(idx)
                    break

    # Identify ground shunts (connected to Node 0)
    for idx, c in enumerate(twoterms):
        if idx in placed_indices:
            continue
        ns = c.get('nodes', [])
        if len(ns) >= 2 and (ns[0] == 0 or ns[1] == 0):
            active_node = ns[0] if ns[1] == 0 else ns[1]
            shunt_comps.append((idx, c, active_node))
            placed_indices.add(idx)

    # Identify input bypass components (both nodes on forward_path, neither is op_out or op_in_minus if follower)
    output_nodes = {op_out, 2}
    if has_opamp and op_in_minus == op_out:
        output_nodes.add(op_in_minus)

    for idx, c in enumerate(twoterms):
        if idx in placed_indices:
            continue
        ns = c.get('nodes', [])
        if len(ns) >= 2:
            u, v = ns[0], ns[1]
            if u not in [3, 4] and v not in [3, 4]:
                if u in forward_path and v in forward_path and u not in output_nodes and v not in output_nodes:
                    input_bypass_comps.append((idx, c, u, v))
                    placed_indices.add(idx)

    # All remaining inter-node branches belong in the upper feedback ladder
    for idx, c in enumerate(twoterms):
        if idx in placed_indices:
            continue
        ns = c.get('nodes', [])
        if len(ns) >= 2:
            u, v = ns[0], ns[1]
            if u not in [3, 4] and v not in [3, 4]:
                bridge_comps.append((idx, c, u, v))
                placed_indices.add(idx)

    # 5. Staggering: for any node on forward_path that receives an input bypass and also has a Ground shunt,
    # define an inlet coordinate node_in_x[v] = node_x[v] - 0.8 to give complete horizontal separation.
    nodes_with_shunts = set(s[2] for s in shunt_comps)
    node_in_x = {}
    for n in forward_path:
        node_in_x[n] = node_x[n]

    for idx, c, u, v in input_bypass_comps:
        # Determine rightmost node
        right_n = v if node_x[v] > node_x[u] else u
        if right_n in nodes_with_shunts:
            node_in_x[right_n] = node_x[right_n] - 0.8

    # 6. Upper Bridging Ladder: Sort feedback branches by span length
    bridge_comps.sort(key=lambda b: abs(node_x.get(b[2], 0) - node_x.get(b[3], 0)))

    track_y = {}
    curr_y = 3.0
    dy_track = 1.4
    for b in bridge_comps:
        track_y[b[0]] = curr_y
        curr_y += dy_track

    # Single vertical riser trunk bounds per node column (EXCLUSIVELY upward for bridge_comps!)
    node_verticals = defaultdict(lambda: {'max_y': 0.0, 'points': set()})
    for b in bridge_comps:
        idx, c, u, v = b
        y = track_y[idx]
        node_verticals[u]['max_y'] = max(node_verticals[u]['max_y'], y)
        node_verticals[u]['points'].add(y)
        node_verticals[v]['max_y'] = max(node_verticals[v]['max_y'], y)
        node_verticals[v]['points'].add(y)

    # 7. Lower Input Bypass Tracks: Sort by span length
    input_bypass_comps.sort(key=lambda b: abs(node_x.get(b[2], 0) - node_x.get(b[3], 0)))
    bypass_y = {}
    curr_bypass_y = -1.8

    for b in input_bypass_comps:
        idx, c, u, v = b
        x1 = min(node_x[u], node_x[v])
        x2 = max(node_x[u], node_x[v])
        # Check if there are intermediate Ground shunts
        has_intermediate_shunts = any(x1 < node_x.get(s[2], -999) < x2 for s in shunt_comps)
        by = curr_bypass_y if not has_intermediate_shunts else (curr_bypass_y - 1.0)
        bypass_y[idx] = by
        curr_bypass_y = by - 1.0

    # -----------------------------------------------------------
    # START DRAWING (AoE Publication Quality)
    # -----------------------------------------------------------
    outfile = Path(outfile)
    outfile.parent.mkdir(parents=True, exist_ok=True)

    with schemdraw.Drawing(file=str(outfile), show=False) as d:
        d.config(unit=2.5, fontsize=10, inches_per_unit=0.45)

        # A. Input Terminal (Node 1) on Left
        x_in = node_x.get(1, 0.0)
        d += elm.Line().right().at((x_in - 0.8, 0)).to((x_in, 0)).label('IN (Node 1)', loc='left')
        d.add(elm.Dot().at((x_in, 0)))

        # B. Forward Backbone Components (strictly horizontal, loc='bottom')
        for idx, c, u, v in backbone_comps:
            x_u = node_x[u]
            x_v = node_in_x[v]  # Connects to inlet if staggered
            el, val = get_element(c['comp_type'], c['value'])
            lbl = f"{c['comp_type']}{c['id']}\n{val}"
            d += el.right().at((x_u, 0)).to((x_v, 0)).label(lbl, loc='bottom')
            d.add(elm.Dot().at((x_u, 0)))
            d.add(elm.Dot().at((x_v, 0)))

            # If staggered inlet, draw horizontal bus wire to base node_x[v]
            if x_v != node_x[v]:
                d += elm.Line().right().at((x_v, 0)).to((node_x[v], 0))
                d.add(elm.Dot().at((node_x[v], 0)))

        # C. Input Bypass Components (in lower half Y < 0, cleanly below series resistors)
        for idx, c, u, v in input_bypass_comps:
            x_left_node = u if node_x[u] < node_x[v] else v
            x_right_node = v if node_x[u] < node_x[v] else u

            x_start = node_x[x_left_node]
            x_end = node_in_x[x_right_node]
            y_b = bypass_y[idx]

            # Vertical wire dropping down from x_start
            d += elm.Line().down().at((x_start, 0)).to((x_start, y_b))
            d.add(elm.Dot().at((x_start, 0)))
            d.add(elm.Dot().at((x_start, y_b)))

            # Component element horizontally
            el, val = get_element(c['comp_type'], c['value'])
            lbl = f"{c['comp_type']}{c['id']}\n{val}"

            if c['comp_type'] == 'D':
                # Check orientation
                if c['nodes'][0] == x_left_node:
                    # Anode at left, Cathode at right
                    d += el.right().at((x_start, y_b)).to((x_end, y_b)).label(lbl, loc='bottom')
                else:
                    # Anode at right, Cathode at left
                    d += el.right().reverse().at((x_start, y_b)).to((x_end, y_b)).label(lbl, loc='bottom')
            else:
                d += el.right().at((x_start, y_b)).to((x_end, y_b)).label(lbl, loc='bottom')

            # Vertical wire rising up to x_end
            d += elm.Line().up().at((x_end, y_b)).to((x_end, 0))
            d.add(elm.Dot().at((x_end, 0)))

        # D. Ground Shunts (Strictly downward, isolated in Y < 0 half)
        shunts_by_node = defaultdict(list)
        for idx, c, u in shunt_comps:
            shunts_by_node[u].append((idx, c))

        for u, comps in shunts_by_node.items():
            base_x = node_x.get(u, 0.0)
            n_comps = len(comps)
            for s_idx, (idx, c) in enumerate(comps):
                if n_comps == 1:
                    sx = base_x
                else:
                    offset = (s_idx - (n_comps - 1) / 2.0) * 1.2
                    sx = base_x + offset
                    d += elm.Line().right().at((min(base_x, sx), 0)).to((max(base_x, sx), 0))
                    d.add(elm.Dot().at((base_x, 0)))

                el, val = get_element(c['comp_type'], c['value'])
                lbl = f"{c['comp_type']}{c['id']}\n{val}"
                d += el.down().at((sx, 0)).length(1.8).label(lbl, loc='bottom')
                d += elm.Ground().at((sx, -1.8))
                d.add(elm.Dot().at((sx, 0)))

        # E. Op-Amp Stage (Strictly pointing rightward, theta=0)
        if has_opamp:
            x_plus = node_x[op_in_plus]
            x_out = node_x[op_out]

            # Place Op-Amp body pointing right at (op_x, 0.0)
            op = elm.Opamp(flip=False).right().at((op_x, 0.0))
            d.add(op)

            # Connect non-inverting input (+) orthogonally into op.in2 (y = -0.625)
            d += elm.Line().right().at((x_plus, 0)).to((op_x - 0.5, 0))
            d += elm.Line().down().to((op_x - 0.5, op.in2[1]))
            d += elm.Line().right().to(op.in2)

            # Power rails: loc='bottom' places text cleanly to the RIGHT of the vertical stub
            d += elm.Line().up().at(op.vd).length(0.6).label(f'+{vcc:.0f}V (VCC)', loc='bottom')
            d += elm.Line().down().at(op.vs).length(0.6).label(f'{vee:.0f}V (VEE)', loc='bottom')

            # Op-Amp Output line to x_out
            d += elm.Line().right().at(op.out).to((x_out, 0))
            d.add(elm.Dot().at((x_out, 0)))

            # If follower buffer (in_minus == out_node):
            # Clean loop from x_out up to 2.0, left over opamp, down to op.in1 (y = 0.625)
            if op_in_minus == op_out:
                d += elm.Line().up().at((x_out, 0)).to((x_out, 2.0))
                d += elm.Line().left().to((op.in1[0] - 0.5, 2.0))
                d += elm.Line().down().to((op.in1[0] - 0.5, op.in1[1]))
                d += elm.Line().right().to(op.in1)

        # F. Output Terminal (Node 2) on Right
        x_2 = node_x.get(2, current_x)
        d += elm.Line().right().at((x_2, 0)).length(1.0).label('OUT (Node 2)', loc='right')
        d.add(elm.Dot().at((x_2, 0)))

        # G. Single Vertical Trunks for Upper Bridging Nodes (EXCLUSIVELY upward)
        for n, data in node_verticals.items():
            if n not in node_x:
                continue
            xn = node_x[n]
            if data['max_y'] > 0:
                start_y = 2.0 if (has_opamp and n == op_out and op_in_minus == op_out) else 0.0
                d += elm.Line().up().at((xn, start_y)).to((xn, data['max_y']))
                for py in data['points']:
                    d.add(elm.Dot().at((xn, py)))

        # H. Horizontal Bridging Branches (Upper feedback ladder above axis)
        for idx, c, u, v in bridge_comps:
            xu = node_x.get(u, 0.0)
            xv = node_x.get(v, 0.0)
            y = track_y[idx]
            x_start = min(xu, xv)
            x_end = max(xu, xv)

            el, val = get_element(c['comp_type'], c['value'])
            lbl = f"{c['comp_type']}{c['id']}\n{val}"
            d += el.right().at((x_start, y)).to((x_end, y)).label(lbl, loc='top')

    print(f"[OK] AoE Geometric Schematic generated: {outfile}")
    return str(outfile)


def main():
    if len(sys.argv) < 2:
        print("Usage:")
        print("  python scripts/schematic.py <checkpoint.json> <circuit_id> [output.svg]")
        print("  python scripts/schematic.py <circuit.net> [output.svg]")
        sys.exit(1)

    first_arg = sys.argv[1]

    # Mode 1: SPICE Netlist (.net)
    if first_arg.endswith('.net'):
        net_path = Path(first_arg)
        out_svg = sys.argv[2] if len(sys.argv) > 2 else str(net_path.with_suffix('.svg'))
        circuit = parse_netlist_file(net_path)
        render_circuit_schematic(circuit, out_svg, title=net_path.stem)
        return

    # Mode 2: Checkpoint JSON
    if len(sys.argv) < 3:
        print("Error: Circuit ID required when specifying a checkpoint JSON file.")
        sys.exit(1)

    cp_path = Path(first_arg)
    circuit_id = int(sys.argv[2])
    out_svg = sys.argv[3] if len(sys.argv) > 3 else f"bench/circuit_{circuit_id:02}/schematic.svg"

    with open(cp_path, 'r', encoding='utf-8') as f:
        data = json.load(f)

    entries = data.get('archive', {}).get('entries', [])
    if circuit_id >= len(entries):
        print(f"Error: Circuit ID {circuit_id} out of bounds (total: {len(entries)})")
        sys.exit(1)

    circuit = entries[circuit_id]['circuit']
    render_circuit_schematic(circuit, out_svg, title=f"Discovery #{circuit_id}")


if __name__ == '__main__':
    main()
