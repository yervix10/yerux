#include "common.h"
typedef struct { i64 x, y, vx, vy; } P;
DEFLIST(LP, P)
static P step(P p) {
  P q = p;
  q.x = ADD(q.x, q.vx); q.y = ADD(q.y, q.vy); q.vy = ADD(q.vy, 1);
  if (q.x > 1000000) q.vx = SUB(0, q.vx);
  if (q.x < 0) q.vx = SUB(0, q.vx);
  if (q.y > 1000000) q.vy = SUB(0, q.vy);
  if (q.y < 0) q.vy = SUB(0, q.vy);
  return q;
}
int main(void) {
  i64 n = 1000; LP ps = {0}; i64 i = 0;
  while (i < n) {
    P p = { MUL(i, 997) % 1000000, MUL(i, 331) % 1000000, ADD(i % 17, 1), ADD(i % 13, 1) };
    LP_push(&ps, p); i = ADD(i, 1);
  }
  i64 t = 0;
  while (t < 100000) { i = 0; while (i < n) { LP_set(&ps, i, step(LP_get(&ps, i))); i = ADD(i, 1); } t = ADD(t, 1); }
  i64 sum = 0; i = 0;
  while (i < n) { P p = LP_get(&ps, i); sum = ADD(sum, ADD(ADD(p.x, MUL(p.y, 3)), ADD(MUL(p.vx, 5), MUL(p.vy, 7)))); i = ADD(i, 1); }
  printf("%lld\n", sum);
}
