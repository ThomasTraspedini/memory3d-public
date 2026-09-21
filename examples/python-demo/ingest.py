"""Create the deterministic Memory3D Python demo database."""

import json
import pathlib
import sys

from memory3d import Memory

from fixture import NODES, RELATIONS


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: ingest.py DB_PATH", file=sys.stderr)
        return 2

    path = pathlib.Path(sys.argv[1])
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        print(f"error: {path} already exists; remove it before ingest", file=sys.stderr)
        return 2

    with Memory.open(path) as memory:
        stored = [
            memory.add_text(kind, text, importance)
            for kind, text, importance in NODES
        ]
        for source, target, relation, weight in RELATIONS:
            memory.link(stored[source].id, stored[target].id, relation, weight)

    print(
        json.dumps(
            {
                "database": str(path),
                "node_count": len(NODES),
                "relation_count": len(RELATIONS),
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
