/* What the middle-end should emit for bench/non-io/class-poly.mjs.
 * Objects are shape + slots (ADR 0050, landed). The call site `pts[i].dist()` carries a two-entry
 * inline cache keyed by shape; each entry names the resolved method statically, so the call is
 * direct and the body is inlined by clang. `this.x` inside the body is a slot read whose shape
 * guard is already discharged by the call site's own guard. `Math.sqrt` is an intrinsic under a
 * realm epoch guard. Numbers are NaN-boxed. */
#include <stdint.h>
#include <stdio.h>
#include <time.h>
static double now_ms(void){struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec*1e3+t.tv_nsec/1e6;}
#include <stdlib.h>
#include <math.h>
#include <string.h>
typedef uint64_t kiln_val;
#define IS_NUM(v) ((v) < UINT64_C(0xFFF8000000000000))
#define TAG_OBJ UINT64_C(0xFFFA000000000000)
#define PTR(v) ((void*)((v) & UINT64_C(0x0000FFFFFFFFFFFF)))
static inline double unbox(kiln_val v){ double d; memcpy(&d,&v,8); return d; }
static inline kiln_val box(double d){ kiln_val v; memcpy(&v,&d,8); return v; }
typedef struct shape { int id; } shape;
typedef struct object { shape *shape; kiln_val slots[4]; } object;
static shape shape_point = {1}, shape_point3d = {2};
static uint32_t math_epoch = 0;           /* bumped on any write to the Math object */
static const uint32_t math_epoch_at_compile = 0;
__attribute__((noinline)) static kiln_val generic_call_method(kiln_val recv){ (void)recv; abort(); }
__attribute__((noinline)) static kiln_val generic_sqrt(kiln_val v){ return box(sqrt(unbox(v))); }
/* dist bodies, typed on a receiver whose shape the caller has proven */
static inline double point_dist(object *o){
  kiln_val x = o->slots[0], y = o->slots[1];
  if (__builtin_expect(IS_NUM(x) && IS_NUM(y) && math_epoch == math_epoch_at_compile, 1)) {
    double a = unbox(x), b = unbox(y); return sqrt(a*a + b*b);
  }
  return unbox(generic_sqrt(box(unbox(x)*unbox(x)+unbox(y)*unbox(y))));
}
static inline double point3d_dist(object *o){
  kiln_val x = o->slots[0], y = o->slots[1], z = o->slots[2];
  if (__builtin_expect(IS_NUM(x) && IS_NUM(y) && IS_NUM(z) && math_epoch == math_epoch_at_compile, 1)) {
    double a = unbox(x), b = unbox(y), c = unbox(z); return sqrt(a*a + b*b + c*c);
  }
  return unbox(generic_sqrt(box(unbox(x)*unbox(x)+unbox(y)*unbox(y)+unbox(z)*unbox(z))));
}
int main(void){ double samples[20]; int rep; for (rep=0; rep<20; rep++){ double t0=now_ms();
  enum { N = 1000 };
  kiln_val *pts = malloc(N * sizeof *pts);
  for (int i = 0; i < N; i++){
    object *o = calloc(1, sizeof *o);
    o->slots[0] = box(i); o->slots[1] = box(i+1);
    if (i % 2 == 0) o->shape = &shape_point; else { o->shape = &shape_point3d; o->slots[2] = box(i+2); }
    pts[i] = TAG_OBJ | (uint64_t)(uintptr_t)o;
  }
  double sum = 0;
  for (int r = 0; r < 500; r++)
    for (int i = 0; i < N; i++){
      kiln_val recv = pts[i];
      /* site IC: 2 entries, shape -> resolved method, prototype epoch checked via shape */
      if ((recv & UINT64_C(0xFFFF000000000000)) == TAG_OBJ) {
        object *o = PTR(recv);
        if (o->shape == &shape_point)        { sum += point_dist(o);   continue; }
        else if (o->shape == &shape_point3d) { sum += point3d_dist(o); continue; }
      }
      sum += unbox(generic_call_method(recv));
    }
  samples[rep]=now_ms()-t0; if (rep==19) printf("%.0f\n", round(sum)); }
  for (int a=0;a<20;a++) for(int c=a+1;c<20;c++) if (samples[c]<samples[a]){double x=samples[a];samples[a]=samples[c];samples[c]=x;}
  fprintf(stderr, "workload median %.4f ms\n", samples[10]); return 0;
}
