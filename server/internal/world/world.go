// Package world loads the shared map and vehicle definitions (shared/*.json),
// which the Rust client reads too.
package world

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
)

type AABB struct {
	Name string     `json:"name"`
	Min  [3]float32 `json:"min"`
	Max  [3]float32 `json:"max"`
}

type Map struct {
	Name     string `json:"name"`
	HalfSize float32 `json:"half_size"`
	Plots    struct {
		Count  int     `json:"count"`
		Radius float32 `json:"radius"`
		Size   float32 `json:"size"`
	} `json:"plots"`
	Statics []AABB `json:"statics"`
}

type VehicleDef struct {
	Class       string     `json:"class"`
	Seats       int        `json:"seats"`
	HalfExtents [3]float32 `json:"half_extents"`
	Mass        float32    `json:"mass"`
	Health      float32    `json:"health"`
}

type World struct {
	Map      Map
	Vehicles map[string]VehicleDef
}

func Load(dir string) (*World, error) {
	w := &World{}
	if err := readJSON(filepath.Join(dir, "map.json"), &w.Map); err != nil {
		return nil, err
	}
	if err := readJSON(filepath.Join(dir, "vehicles.json"), &w.Vehicles); err != nil {
		return nil, err
	}
	if w.Map.Plots.Count <= 0 {
		return nil, fmt.Errorf("map has no plots")
	}
	return w, nil
}

func readJSON(path string, v any) error {
	b, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	if err := json.Unmarshal(b, v); err != nil {
		return fmt.Errorf("%s: %w", path, err)
	}
	return nil
}

// Plot returns the plot center and yaw. Rotating local +Z by yaw points at the island center.
func (w *World) Plot(i int) (x, z, yaw float32) {
	a := float64(i) * 2 * math.Pi / float64(w.Map.Plots.Count)
	r := float64(w.Map.Plots.Radius)
	return float32(r * math.Sin(a)), float32(r * math.Cos(a)), float32(a + math.Pi)
}

// PlotSpawn is where a player's character appears on its plot (local (0, 1.5, -20)).
func (w *World) PlotSpawn(i int) [3]float32 {
	x, z, yaw := w.Plot(i)
	s, c := math.Sincos(float64(yaw))
	lx, lz := 0.0, -20.0
	// rotation about Y: (x, z) -> (x cos + z sin, -x sin + z cos)
	return [3]float32{x + float32(lx*c+lz*s), 1.5, z + float32(-lx*s+lz*c)}
}

// GroundHeight returns the top of whatever static geometry lies under (x, z) at or below y.
func (w *World) GroundHeight(x, y, z float32) float32 {
	g := float32(0)
	for _, b := range w.Map.Statics {
		if x >= b.Min[0] && x <= b.Max[0] && z >= b.Min[2] && z <= b.Max[2] && b.Max[1] <= y+0.5 && b.Max[1] > g {
			g = b.Max[1]
		}
	}
	return g
}

func (w *World) Vehicle(def string) VehicleDef {
	if v, ok := w.Vehicles[def]; ok {
		return v
	}
	return VehicleDef{Class: "prop", Seats: 0, HalfExtents: [3]float32{0.5, 0.5, 0.5}, Mass: 50, Health: 100}
}
