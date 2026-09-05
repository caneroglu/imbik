import xml.etree.ElementTree as ET

tree = ET.parse("bench/circuit_34/schematic.svg")
root = tree.getroot()

ns = {'svg': 'http://www.w3.org/2000/svg'}
for elem in root.iter('{http://www.w3.org/2000/svg}text'):
    x = float(elem.attrib.get('x', 0))
    y = float(elem.attrib.get('y', 0))
    txt = " ".join("".join(elem.itertext()).split())
    print(f"{txt:25s} at x={x:7.2f}, y={y:7.2f}")

