"""Recall from an existing Memory3D Python demo database."""

import json
import sys

from memory3d import Memory

from fixture import RECALL_QUERY


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: recall.py DB_PATH", file=sys.stderr)
        return 2

    with Memory.open(sys.argv[1]) as memory:
        report = memory.activate(
            RECALL_QUERY,
            hops=3,
            seed_limit=1,
            limit=10,
            max_visited_nodes=64,
            max_visited_edges=128,
        )

    print(
        json.dumps(
            {
                "query": RECALL_QUERY,
                "stats": {
                    "seeds": report.stats.seeds,
                    "visited_nodes": report.stats.visited_nodes,
                    "visited_edges": report.stats.visited_edges,
                },
                "results": [
                    {
                        "id": result.node.id,
                        "text": result.node.text,
                        "score": result.score,
                        "hops": result.hops,
                        "path": {
                            "seed": result.path.seed,
                            "steps": [
                                {
                                    "source": step.source,
                                    "relation": step.relation,
                                    "weight": step.weight,
                                    "target": step.target,
                                }
                                for step in result.path.steps
                            ],
                        },
                    }
                    for result in report.results
                ],
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
