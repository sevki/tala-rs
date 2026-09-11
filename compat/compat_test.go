// Package compat checks upstream D2's d2talalayout engine for invariant
// drift. It does not invoke tala-rs or compare against its output — it runs
// the same fixtures (shared with the Rust golden tests in
// ../tests/fixtures) through the real upstream engine and asserts the same
// structural invariants that ../tests/golden.rs asserts for tala-rs: no
// unrelated node overlaps, orthogonal edge routing, and children contained
// within their parent's bounds. tala-rs regressions against those
// invariants only show up in the Rust suite; this package only catches
// upstream drifting away from them.
package compat

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/d2lang/d2/d2graph"
	"github.com/d2lang/d2/d2layouts/d2talalayout"
	"github.com/d2lang/d2/lib/geo"
)

type fixtureNode struct {
	ID     string `json:"id"`
	Width  int    `json:"width"`
	Height int    `json:"height"`
	Parent string `json:"parent"`
	Fixed  bool   `json:"fixed"`
	X      *int   `json:"x"`
	Y      *int   `json:"y"`
}

type fixtureEdge struct {
	Source string `json:"source"`
	Target string `json:"target"`
}

type fixture struct {
	Seeds          []int64       `json:"seeds"`
	MaxConcurrency int           `json:"max_concurrency"`
	Nodes          []fixtureNode `json:"nodes"`
	Edges          []fixtureEdge `json:"edges"`
}

func fixturesDir(t *testing.T) string {
	t.Helper()
	dir, err := filepath.Abs(filepath.Join("..", "tests", "fixtures"))
	if err != nil {
		t.Fatalf("resolve fixtures dir: %v", err)
	}
	return dir
}

func loadFixtures(t *testing.T) map[string]fixture {
	t.Helper()
	dir := fixturesDir(t)
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("read %s: %v", dir, err)
	}
	fixtures := make(map[string]fixture)
	for _, entry := range entries {
		if entry.IsDir() || filepath.Ext(entry.Name()) != ".json" {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(dir, entry.Name()))
		if err != nil {
			t.Fatalf("read %s: %v", entry.Name(), err)
		}
		var f fixture
		if err := json.Unmarshal(raw, &f); err != nil {
			t.Fatalf("parse %s: %v", entry.Name(), err)
		}
		name := entry.Name()[:len(entry.Name())-len(filepath.Ext(entry.Name()))]
		fixtures[name] = f
	}
	if len(fixtures) == 0 {
		t.Fatalf("expected at least one fixture in %s", dir)
	}
	return fixtures
}

// buildGraph requires every node's parent (if any) to already have been
// declared earlier in f.Nodes, matching the convention used by the shared
// fixtures.
func buildGraph(t *testing.T, f fixture) *d2graph.Graph {
	t.Helper()
	g := d2graph.NewGraph()
	objects := make(map[string]*d2graph.Object, len(f.Nodes))

	for _, n := range f.Nodes {
		parent := g.Root
		if n.Parent != "" {
			p, ok := objects[n.Parent]
			if !ok {
				t.Fatalf("node %q references parent %q before it is declared", n.ID, n.Parent)
			}
			parent = p
		}
		obj := &d2graph.Object{
			Graph:      g,
			Parent:     parent,
			ID:         n.ID,
			IDVal:      n.ID,
			Box:        geo.NewBox(geo.NewPoint(0, 0), float64(n.Width), float64(n.Height)),
			Children:   make(map[string]*d2graph.Object),
			Attributes: d2graph.Attributes{},
		}
		if n.Fixed {
			if n.X == nil || n.Y == nil {
				t.Fatalf("fixed node %q must specify x and y", n.ID)
			}
			obj.Left = &d2graph.Scalar{Value: fmt.Sprintf("%d", *n.X)}
			obj.Top = &d2graph.Scalar{Value: fmt.Sprintf("%d", *n.Y)}
		}
		parent.Children[strings.ToLower(obj.ID)] = obj
		parent.ChildrenArray = append(parent.ChildrenArray, obj)
		g.Objects = append(g.Objects, obj)
		objects[n.ID] = obj
	}

	for _, e := range f.Edges {
		src, ok := objects[e.Source]
		if !ok {
			t.Fatalf("edge references unknown source %q", e.Source)
		}
		dst, ok := objects[e.Target]
		if !ok {
			t.Fatalf("edge references unknown target %q", e.Target)
		}
		g.Edges = append(g.Edges, &d2graph.Edge{Src: src, Dst: dst})
	}

	return g
}

func runLayout(t *testing.T, f fixture) *d2graph.Graph {
	t.Helper()
	g := buildGraph(t, f)
	opts := d2talalayout.DefaultOptions()
	opts.Seeds = f.Seeds
	opts.MaxConcurrency = f.MaxConcurrency
	ctx := context.Background()
	if err := d2talalayout.Layout(ctx, g, &opts); err != nil {
		t.Fatalf("layout: %v", err)
	}
	if err := d2talalayout.RouteEdges(ctx, g, g.Edges); err != nil {
		t.Fatalf("route edges: %v", err)
	}
	return g
}

type rect struct {
	x, y, w, h float64
}

func (r rect) right() float64  { return r.x + r.w }
func (r rect) bottom() float64 { return r.y + r.h }

func (r rect) intersects(o rect) bool {
	return r.x < o.right() && r.right() > o.x && r.y < o.bottom() && r.bottom() > o.y
}

func (r rect) contains(o rect) bool {
	return r.x <= o.x && r.y <= o.y && r.right() >= o.right() && r.bottom() >= o.bottom()
}

func objectRect(o *d2graph.Object) rect {
	return rect{x: o.TopLeft.X, y: o.TopLeft.Y, w: o.Width, h: o.Height}
}

func isAncestor(ancestor, node *d2graph.Object) bool {
	for current := node.Parent; current != nil; current = current.Parent {
		if current == ancestor {
			return true
		}
	}
	return false
}

func TestFixturesSatisfyStructuralInvariants(t *testing.T) {
	for name, f := range loadFixtures(t) {
		f := f
		t.Run(name, func(t *testing.T) {
			g := runLayout(t, f)

			for i := 0; i < len(g.Objects); i++ {
				for j := i + 1; j < len(g.Objects); j++ {
					left, right := g.Objects[i], g.Objects[j]
					if isAncestor(left, right) || isAncestor(right, left) {
						continue
					}
					if objectRect(left).intersects(objectRect(right)) {
						t.Errorf("unrelated nodes %q and %q overlap", left.ID, right.ID)
					}
				}
			}

			for _, edge := range g.Edges {
				if len(edge.Route) < 2 {
					t.Errorf("edge %q->%q has fewer than two route points", edge.Src.ID, edge.Dst.ID)
					continue
				}
				for k := 0; k+1 < len(edge.Route); k++ {
					a, b := edge.Route[k], edge.Route[k+1]
					if a.X != b.X && a.Y != b.Y {
						t.Errorf("edge %q->%q has a non-orthogonal segment", edge.Src.ID, edge.Dst.ID)
					}
				}
			}

			for _, obj := range g.Objects {
				if obj.Parent == nil || obj.Parent == g.Root {
					continue
				}
				if !objectRect(obj.Parent).contains(objectRect(obj)) {
					t.Errorf("child %q is not contained within parent %q", obj.ID, obj.Parent.ID)
				}
			}
		})
	}
}
