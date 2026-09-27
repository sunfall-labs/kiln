// The Go side of bench/concurrency: the same six workloads as the JavaScript programs beside it,
// with a goroutine where they have a worker and a channel where they have a port.
//
// Every subcommand prints JSON lines of the same shape the JavaScript programs print, so run.py
// reads both the same way and compares their checksums. Go is the reference for "what the
// language gives you for free": goroutines share memory, so a "transfer" is sending a slice
// header and a "shared dataset" is a slice every goroutine reads.
//
// usage: concurrency-go <workload> [args...]
package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

func emit(fields map[string]any) {
	line, _ := json.Marshal(fields)
	fmt.Println(string(line))
}

func rssKiB() int64 {
	data, err := os.ReadFile("/proc/self/status")
	if err != nil {
		return -1
	}
	for _, line := range strings.Split(string(data), "\n") {
		if strings.HasPrefix(line, "VmRSS:") {
			fields := strings.Fields(line)
			n, _ := strconv.ParseInt(fields[1], 10, 64)
			return n
		}
	}
	return -1
}

func median(xs []float64) float64 {
	s := append([]float64(nil), xs...)
	sort.Float64s(s)
	return s[len(s)/2]
}

func ms(d time.Duration) float64 { return float64(d.Nanoseconds()) / 1e6 }

func argInt(i int, def int) int {
	if len(os.Args) > i {
		n, err := strconv.Atoi(os.Args[i])
		if err == nil {
			return n
		}
	}
	return def
}

// ---- 1. start: a goroutine that sends one message back ------------------------------------

func startCost(checked int) {
	const warm = 20000
	reply := func(i int) int {
		ch := make(chan int)
		go func() { ch <- i + 1 }()
		return <-ch
	}
	t := time.Now()
	sum := reply(0)
	cold := ms(time.Since(t))
	// The checksum covers the first 1 + checked replies, as the JavaScript programs' 1 cold +
	// `checked` warm starts do; the timing below runs many more.
	for i := 1; i <= checked; i++ {
		sum += reply(i)
	}
	samples := []float64{}
	for round := 0; round < 9; round++ {
		t := time.Now()
		for i := 1; i <= warm; i++ {
			reply(i)
		}
		samples = append(samples, ms(time.Since(t))/warm)
	}
	emit(map[string]any{"case": "start", "cold_ms": cold, "median_ms": median(samples), "checksum": strconv.Itoa(sum)})
}

func startMemory(n int) {
	runtime.GC()
	before := rssKiB()
	release := make(chan struct{})
	ready := make(chan int)
	for i := 0; i < n; i++ {
		go func(i int) {
			ready <- i + 1
			<-release
		}(i)
	}
	sum := 0
	for i := 0; i < n; i++ {
		sum += <-ready
	}
	after := rssKiB()
	close(release)
	emit(map[string]any{"case": fmt.Sprintf("mem-%d", n), "rss_before_kib": before, "rss_kib": after,
		"per_worker_kib": float64(after-before) / float64(n), "checksum": strconv.Itoa(sum)})
}

// ---- 2. messages ----------------------------------------------------------------------------

type Small struct {
	ID    int
	Name  string
	Tags  []string
	Qty   int
	Price float64
}

type Item struct {
	ID    int
	Name  string
	Tags  []string
	Price float64
	Cat   *Category
}

type Category struct {
	Code  int
	Label string
}

// The same graph the JavaScript programs build: 6000 items with a name, two tags and a price,
// each pointing at one of 13 shared category objects (a graph, not a tree), about 1 MB once
// structured-cloned.
func buildGraph() []*Item {
	cats := make([]*Category, 13)
	for i := range cats {
		cats[i] = &Category{Code: i * 3, Label: "category-" + strconv.Itoa(i)}
	}
	items := make([]*Item, 6000)
	for i := range items {
		items[i] = &Item{ID: i, Name: "item-" + strconv.Itoa(i), Tags: []string{"t" + strconv.Itoa(i%13), "group-" + strconv.Itoa(i%97)}, Price: float64(i%1000) / 4, Cat: cats[i%13]}
	}
	return items
}

func walkGraph(items []*Item) int {
	sum := 0
	for _, it := range items {
		sum += it.ID + len(it.Name) + len(it.Tags[1]) + int(it.Price*4) + it.Cat.Code
	}
	return sum
}

func deepCopy(items []*Item) []*Item {
	out := make([]*Item, len(items))
	cats := map[*Category]*Category{}
	for i, it := range items {
		c, ok := cats[it.Cat]
		if !ok {
			c = &Category{Code: it.Cat.Code, Label: strings.Clone(it.Cat.Label)}
			cats[it.Cat] = c
		}
		out[i] = &Item{ID: it.ID, Name: strings.Clone(it.Name), Tags: []string{strings.Clone(it.Tags[0]), strings.Clone(it.Tags[1])}, Price: it.Price, Cat: c}
	}
	return out
}

func timedPerOp(budget time.Duration, batch int, op func()) float64 {
	samples := []float64{}
	deadline := time.Now().Add(budget)
	for time.Now().Before(deadline) || len(samples) < 3 {
		t := time.Now()
		for i := 0; i < batch; i++ {
			op()
		}
		samples = append(samples, ms(time.Since(t))/float64(batch))
	}
	return median(samples)
}

func messages(budgetMs int) {
	budget := time.Duration(budgetMs) * time.Millisecond

	// small ping-pong: an echo goroutine on two unbuffered channels.
	{
		in, out := make(chan Small), make(chan Small)
		go func() {
			for m := range in {
				out <- m
			}
		}()
		sum := 0
		i := 0
		per := timedPerOp(budget, 2000, func() {
			in <- Small{ID: i, Name: "order", Tags: []string{"a", "b"}, Qty: 3, Price: 9.5}
			if (<-out).ID != i {
				panic("echo mismatch")
			}
			sum++
			i++
		})
		close(in)
		emit(map[string]any{"case": "small-pingpong", "us_per_op": per * 1000, "ops_per_s": 1000 / per, "checksum": "ok"})
		_ = sum
	}
	// small one-way: 100k messages down a buffered channel, one reply at the end.
	{
		const count = 100000
		samples := []float64{}
		var total int
		deadline := time.Now().Add(budget)
		for time.Now().Before(deadline) || len(samples) < 3 {
			in := make(chan Small, 1024)
			done := make(chan int)
			go func() {
				s := 0
				for m := range in {
					s += m.Qty
				}
				done <- s
			}()
			t := time.Now()
			for i := 0; i < count; i++ {
				in <- Small{ID: i, Name: "order", Tags: []string{"a", "b"}, Qty: 3, Price: 9.5}
			}
			close(in)
			total = <-done
			samples = append(samples, ms(time.Since(t)))
		}
		m := median(samples)
		emit(map[string]any{"case": "small-oneway", "msgs_per_s": count / (m / 1000), "checksum": strconv.Itoa(total)})
	}
	// 1 MB object graph: by reference (what a goroutine does) and deep-copied (what a worker does).
	graph := buildGraph()
	for _, variant := range []string{"graph-1mb-ref", "graph-1mb"} {
		in, out := make(chan []*Item), make(chan int)
		go func() {
			for g := range in {
				out <- walkGraph(g)
			}
		}()
		sum := 0
		per := timedPerOp(budget, 4, func() {
			if variant == "graph-1mb-ref" {
				in <- graph
			} else {
				in <- deepCopy(graph)
			}
			sum = <-out
		})
		close(in)
		emit(map[string]any{"case": variant, "ms_per_op": per, "checksum": strconv.Itoa(sum)})
	}
	// 16 MB buffer, round trip: copied each way, or the slice handed over and back.
	buf := make([]byte, 16<<20)
	for i := range buf {
		buf[i] = byte(i*7 + i>>12)
	}
	touch := func(b []byte) int {
		s := 0
		for i := 0; i < len(b); i += 4096 {
			s += int(b[i])
		}
		return s
	}
	for _, variant := range []string{"ab16-copy", "ab16-transfer"} {
		in, out := make(chan []byte), make(chan []byte)
		var check int
		go func() {
			for b := range in {
				check = touch(b)
				if variant == "ab16-copy" {
					c := make([]byte, len(b))
					copy(c, b)
					out <- c
				} else {
					out <- b
				}
			}
		}()
		per := timedPerOp(budget, 2, func() {
			if variant == "ab16-copy" {
				c := make([]byte, len(buf))
				copy(c, buf)
				in <- c
			} else {
				in <- buf
			}
			buf = <-out
		})
		close(in)
		emit(map[string]any{"case": variant, "ms_per_op": per, "checksum": strconv.Itoa(check)})
	}
}

// ---- 3. a shared read-only table -------------------------------------------------------------

const tableLen = 50 * 1024 * 1024 / 8

func lookups(table []float64, seed uint32, q int) float64 {
	x := seed
	sum := 0.0
	n := uint32(len(table))
	for i := 0; i < q; i++ {
		x = x*1664525 + 1013904223
		sum += table[x%n]
	}
	return sum
}

func shared(workers, queries int) {
	t := time.Now()
	table := make([]float64, tableLen)
	for i := range table {
		table[i] = float64((uint64(i) * 2654435761) % 1000003)
	}
	build := ms(time.Since(t))
	runtime.GC()
	var setup, query float64
	var total float64
	samples := []float64{}
	for round := 0; round < 3; round++ {
		t = time.Now()
		results := make([]float64, workers)
		var wg sync.WaitGroup
		for w := 0; w < workers; w++ {
			wg.Add(1)
			go func(w int) {
				defer wg.Done()
				results[w] = lookups(table, uint32(w+1), queries)
			}(w)
		}
		setup = 0 // nothing to distribute: every goroutine reads the same slice
		wg.Wait()
		total = 0
		for _, r := range results {
			total += r
		}
		samples = append(samples, ms(time.Since(t)))
	}
	query = median(samples)
	emit(map[string]any{"case": fmt.Sprintf("shared-%d", workers), "build_ms": build, "setup_ms": setup, "query_ms": query,
		"rss_kib": rssKiB(), "checksum": strconv.FormatFloat(total, 'f', 0, 64)})
}

// ---- 4. many tasks ---------------------------------------------------------------------------

func tasks(n, ticks, interval int) {
	var total int64
	var wg sync.WaitGroup
	t := time.Now()
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for k := 0; k < ticks; k++ {
				time.Sleep(time.Duration(interval) * time.Millisecond)
				atomic.AddInt64(&total, int64(id%7+1))
			}
		}(i)
	}
	wg.Wait()
	wall := ms(time.Since(t))
	emit(map[string]any{"case": fmt.Sprintf("tasks-%d", n), "wall_ms": wall, "ticks_per_s": float64(n*ticks) / (wall / 1000),
		"checksum": strconv.FormatInt(total, 10)})
}

// ---- 5. data-parallel map --------------------------------------------------------------------

func f(x float64) float64 {
	y := x
	for k := 0.0; k < 16; k++ {
		y = float64(int64(y*31+k) % 1000003)
	}
	return y
}

func fLight(x float64) float64 { return float64(int64(x*3+1) % 1000003) }

func pmap(n, reps int, light bool) {
	input := make([]float64, n)
	for i := range input {
		input[i] = float64(i)
	}
	procs := runtime.GOMAXPROCS(0)
	for _, variant := range []string{"map", "parallel"} {
		samples := []float64{}
		var sum float64
		var cold float64
		for r := 0; r <= reps; r++ {
			t := time.Now()
			out := make([]float64, n)
			if variant == "map" {
				for i, x := range input {
					if light {
						out[i] = fLight(x)
					} else {
						out[i] = f(x)
					}
				}
			} else {
				var wg sync.WaitGroup
				chunk := (n + procs - 1) / procs
				for p := 0; p < procs; p++ {
					lo, hi := p*chunk, min((p+1)*chunk, n)
					wg.Add(1)
					go func() {
						defer wg.Done()
						for i := lo; i < hi; i++ {
							if light {
								out[i] = fLight(input[i])
							} else {
								out[i] = f(input[i])
							}
						}
					}()
				}
				wg.Wait()
			}
			d := ms(time.Since(t))
			sum = 0
			for _, v := range out {
				sum += v
			}
			if r == 0 {
				cold = d
			} else {
				samples = append(samples, d)
			}
		}
		name := "pmap-" + variant
		if light {
			name = "pmap-light-" + variant
		}
		emit(map[string]any{"case": name, "cold_ms": cold, "median_ms": median(samples), "threads": map[bool]int{true: 1, false: procs}[variant == "map"],
			"checksum": strconv.FormatFloat(sum, 'f', 0, 64)})
	}
}

// ---- 6. server -------------------------------------------------------------------------------

func serve(port int) {
	body := []byte("hello from kiln")
	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "text/plain")
		w.Write(body)
	})
	fmt.Println("listening")
	if err := http.ListenAndServe("127.0.0.1:"+strconv.Itoa(port), nil); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: concurrency-go start [K]|mem N|messages MS|shared N Q|tasks N TICKS MS|pmap N REPS [light]|serve PORT")
		os.Exit(2)
	}
	switch os.Args[1] {
	case "start":
		startCost(argInt(2, 20))
	case "mem":
		startMemory(argInt(2, 1))
	case "messages":
		messages(argInt(2, 1000))
	case "shared":
		shared(argInt(2, 4), argInt(3, 2000000))
	case "tasks":
		tasks(argInt(2, 1000), argInt(3, 10), argInt(4, 10))
	case "pmap":
		pmap(argInt(2, 10000000), argInt(3, 3), len(os.Args) > 4 && os.Args[4] == "light")
	case "serve":
		serve(argInt(2, 18190))
	default:
		fmt.Fprintln(os.Stderr, "unknown workload", os.Args[1])
		os.Exit(2)
	}
}
