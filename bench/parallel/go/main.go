// The Go column of bench/parallel: each workload with goroutines over sub-slices, one per
// GOMAXPROCS (every core by default), the way a Go programmer would split it.
//
//	go run . <workload> <n> <budget-ms>
//
// It builds the inputs from the same hash as js/common.mjs, times itself the way
// bench/harness.mjs does (a cold call reported apart, a batch calibrated to outlast 50 ms, the
// median of warm batches until the budget is spent), and prints the same JSON line, so run.py
// reads every runtime alike. Like the worker_threads pool, the pipelines are one fused loop per
// goroutine, and sort is per-goroutine sorts merged pairwise in parallel.
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"runtime"
	"slices"
	"sort"
	"strconv"
	"sync"
	"time"
)

func hash(i int) uint32 { return uint32(i+1) * 2654435761 }

func floats(n int) []float64 {
	out := make([]float64, n)
	for i := range out {
		out[i] = float64(hash(i)) / 4294967296 * 1000
	}
	return out
}

func ints(n int) []float64 {
	out := make([]float64, n)
	for i := range out {
		out[i] = float64(hash(i) % 100003)
	}
	return out
}

func qtys(n int) []float64 {
	out := make([]float64, n)
	for i := range out {
		out[i] = float64(1 + (hash(i)>>7)%9)
	}
	return out
}

func ratings(n int) []float64 {
	out := make([]float64, n)
	for i := range out {
		out[i] = float64(1 + (hash(i)>>13)%5)
	}
	return out
}

func num(v float64) string { return strconv.FormatFloat(v, 'g', -1, 64) }

func sumChecksum(values []float64) string {
	sum := 0.0
	for _, v := range values {
		sum += v
	}
	return fmt.Sprintf("%d:%s", len(values), num(sum))
}

func sortChecksum(values []float64) string {
	sum := 0.0
	for i := 0; i < len(values); i += 997 {
		sum += values[i] * float64(i%13+1)
	}
	return fmt.Sprintf("%d:%s:%s:%s", len(values), num(values[0]), num(values[len(values)-1]), num(sum))
}

func valueChecksum(v float64) string { return "1:" + num(v) }

var procs = runtime.GOMAXPROCS(0)

// bounds splits [0, n) into procs contiguous ranges.
func bounds(n int) []int {
	b := make([]int, procs+1)
	for w := 0; w <= procs; w++ {
		b[w] = n * w / procs
	}
	return b
}

// each runs body(w, lo, hi) for every range on its own goroutine and waits.
func each(b []int, body func(w, lo, hi int)) {
	var wg sync.WaitGroup
	for w := 0; w+1 < len(b); w++ {
		wg.Add(1)
		go func(w int) {
			defer wg.Done()
			body(w, b[w], b[w+1])
		}(w)
	}
	wg.Wait()
}

func sumParts(parts []float64) float64 {
	total := 0.0
	for _, p := range parts {
		total += p
	}
	return total
}

func merge(from, to []float64, lo, mid, hi int) {
	i, j, k := lo, mid, lo
	for i < mid && j < hi {
		if from[j] < from[i] {
			to[k] = from[j]
			j++
		} else {
			to[k] = from[i]
			i++
		}
		k++
	}
	k += copy(to[k:], from[i:mid])
	copy(to[k:], from[j:hi])
}

// parallelSort copies xs, sorts each range on its goroutine, and merges runs pairwise, each merge
// of a level on its own goroutine.
func parallelSort(xs []float64, sortRun func([]float64)) []float64 {
	b := bounds(len(xs))
	a := make([]float64, len(xs))
	tmp := make([]float64, len(xs))
	each(b, func(_, lo, hi int) {
		copy(a[lo:hi], xs[lo:hi])
		sortRun(a[lo:hi])
	})
	runs := b
	for len(runs) > 2 {
		next := []int{0}
		var wg sync.WaitGroup
		for r := 0; r+1 < len(runs); r += 2 {
			lo, mid, hi := runs[r], runs[r+1], runs[r+1]
			if r+2 < len(runs) {
				hi = runs[r+2]
			}
			next = append(next, hi)
			wg.Add(1)
			go func(from, to []float64) {
				defer wg.Done()
				merge(from, to, lo, mid, hi)
			}(a, tmp)
		}
		wg.Wait()
		a, tmp = tmp, a
		runs = next
	}
	return a
}

type record struct{ price, qty, rating float64 }

// keyed is sort-records' element: an id and the key it is sorted by.
type keyed struct {
	id    int
	price float64
}

// parallelSortKeyed is parallelSort for records by price, stable: runs sorted on their
// goroutines, then merged pairwise level by level, each merge on its own goroutine.
func parallelSortKeyed(xs []keyed) []keyed {
	byPrice := func(a, b keyed) int {
		switch d := a.price - b.price; {
		case d < 0:
			return -1
		case d > 0:
			return 1
		}
		return 0
	}
	b := bounds(len(xs))
	a := make([]keyed, len(xs))
	tmp := make([]keyed, len(xs))
	each(b, func(_, lo, hi int) {
		copy(a[lo:hi], xs[lo:hi])
		slices.SortStableFunc(a[lo:hi], byPrice)
	})
	runs := b
	for len(runs) > 2 {
		next := []int{0}
		var wg sync.WaitGroup
		for r := 0; r+1 < len(runs); r += 2 {
			lo, mid, hi := runs[r], runs[r+1], runs[r+1]
			if r+2 < len(runs) {
				hi = runs[r+2]
			}
			next = append(next, hi)
			wg.Add(1)
			go func(from, to []keyed) {
				defer wg.Done()
				i, j, k := lo, mid, lo
				for i < mid && j < hi {
					if from[j].price < from[i].price {
						to[k] = from[j]
						j++
					} else {
						to[k] = from[i]
						i++
					}
					k++
				}
				k += copy(to[k:], from[i:mid])
				copy(to[k:], from[j:hi])
			}(a, tmp)
		}
		wg.Wait()
		a, tmp = tmp, a
		runs = next
	}
	return a
}

func ms(d time.Duration) float64 { return float64(d.Nanoseconds()) / 1e6 }

// bench is bench/harness.mjs's timedSync.
func bench(name string, budgetMs float64, work func() any, checksum func(any) string) {
	started := time.Now()
	sink := work()
	cold := ms(time.Since(started))
	batch := 1
	for {
		t := time.Now()
		for i := 0; i < batch; i++ {
			sink = work()
		}
		if ms(time.Since(t)) >= 50 || batch >= 1<<16 {
			break
		}
		batch *= 2
	}
	var samples []float64
	deadline := time.Now().Add(time.Duration(budgetMs * float64(time.Millisecond)))
	for time.Now().Before(deadline) {
		t := time.Now()
		for i := 0; i < batch; i++ {
			sink = work()
		}
		samples = append(samples, ms(time.Since(t))/float64(batch))
	}
	sort.Float64s(samples)
	line, _ := json.Marshal(map[string]any{
		"name": name, "mode": "in-process", "cold": cold, "warm_median": samples[len(samples)/2],
		"warm_best": samples[0], "batch": batch, "samples": len(samples), "checksum": checksum(sink),
		"threads": procs,
	})
	fmt.Println(string(line))
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: parallel <workload> [n] [budget-ms]")
		os.Exit(2)
	}
	workload := os.Args[1]
	n, budget := 1000000, 1500.0
	if len(os.Args) > 2 {
		n, _ = strconv.Atoi(os.Args[2])
	}
	if len(os.Args) > 3 {
		budget, _ = strconv.ParseFloat(os.Args[3], 64)
	}
	b := bounds(n)
	sums := func(v any) string { return sumChecksum(v.([]float64)) }
	sorted := func(v any) string { return sortChecksum(v.([]float64)) }
	value := func(v any) string { return valueChecksum(v.(float64)) }
	fold := func(body func(lo, hi int) float64) func() any {
		return func() any {
			parts := make([]float64, procs)
			each(b, func(w, lo, hi int) { parts[w] = body(lo, hi) })
			return sumParts(parts)
		}
	}

	switch workload {
	case "map-light", "map-heavy":
		xs := floats(n)
		heavy := workload == "map-heavy"
		bench(workload, budget, func() any {
			out := make([]float64, n)
			each(b, func(_, lo, hi int) {
				for i := lo; i < hi; i++ {
					x := xs[i]
					if heavy {
						out[i] = math.Sqrt(x*x+1)*math.Sin(x) + math.Cos(x*0.5)
					} else {
						out[i] = x*1.5 + 2
					}
				}
			})
			return out
		}, sums)
	case "filter":
		xs := floats(n)
		bench(workload, budget, func() any {
			parts := make([][]float64, procs)
			each(b, func(w, lo, hi int) {
				kept := make([]float64, 0, (hi-lo)/2)
				for _, x := range xs[lo:hi] {
					if math.Sin(x)*x > 250 {
						kept = append(kept, x)
					}
				}
				parts[w] = kept
			})
			total := 0
			for _, p := range parts {
				total += len(p)
			}
			out := make([]float64, 0, total)
			for _, p := range parts {
				out = append(out, p...)
			}
			return out
		}, sums)
	case "reduce-sum":
		xs := ints(n)
		bench(workload, budget, fold(func(lo, hi int) float64 {
			acc := 0.0
			for _, x := range xs[lo:hi] {
				acc += x
			}
			return acc
		}), value)
	case "reduce-fold":
		xs := ints(n)
		bench(workload, budget, fold(func(lo, hi int) float64 {
			// Integer arithmetic, as a Go programmer would write it: the values are integers
			// below 2^53 throughout, so this is exactly the JavaScript fold.
			var acc int64
			for _, x := range xs[lo:hi] {
				y := int64(x)
				for k := int64(0); k < 8; k++ {
					y = (y*31 + k) % 1000003
				}
				acc += y
			}
			return float64(acc)
		}), value)
	case "sort-typed":
		xs := floats(n)
		bench(workload, budget, func() any { return parallelSort(xs, slices.Sort[[]float64]) }, sorted)
	case "sort-array":
		xs := floats(n)
		byDifference := func(run []float64) {
			slices.SortStableFunc(run, func(a, b float64) int {
				switch d := a - b; {
				case d < 0:
					return -1
				case d > 0:
					return 1
				}
				return 0
			})
		}
		bench(workload, budget, func() any { return parallelSort(xs, byDifference) }, sorted)
	case "sort-records":
		price := floats(n)
		records := make([]keyed, n)
		for i := range records {
			records[i] = keyed{i, price[i]}
		}
		bench(workload, budget, func() any { return parallelSortKeyed(records) }, func(v any) string {
			sorted := v.([]keyed)
			prices := make([]float64, len(sorted))
			for i, r := range sorted {
				prices[i] = r.price
			}
			return sortChecksum(prices)
		})
	case "pipeline-column":
		xs := floats(n)
		bench(workload, budget, fold(func(lo, hi int) float64 {
			acc := 0.0
			for _, x := range xs[lo:hi] {
				if c := math.Floor(x * 1.0873 * 1.2 * 100); c > 50000 {
					acc += c
				}
			}
			return acc
		}), value)
	case "pipeline-records":
		price, qty, rating := floats(n), qtys(n), ratings(n)
		bench(workload, budget, fold(func(lo, hi int) float64 {
			acc := 0.0
			for i := lo; i < hi; i++ {
				if s := math.Floor(price[i] * qty[i] * (5 + rating[i])); s > 20000 {
					acc += s
				}
			}
			return acc
		}), value)
	case "pipeline-objects":
		price, qty, rating := floats(n), qtys(n), ratings(n)
		records := make([]record, n)
		for i := range records {
			records[i] = record{price[i], qty[i], rating[i]}
		}
		bench(workload, budget, fold(func(lo, hi int) float64 {
			acc := 0.0
			for _, r := range records[lo:hi] {
				if s := math.Floor(r.price * r.qty * (5 + r.rating)); s > 20000 {
					acc += s
				}
			}
			return acc
		}), value)
	default:
		fmt.Fprintln(os.Stderr, "unknown workload", workload)
		os.Exit(2)
	}
}
