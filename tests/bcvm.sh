#!/usr/bin/env bash
# examples/real/bcvm: unit tests on both backends; main built on both; the disassembly and the run output (stdout,
# stderr, status) of fixed programs and of a seeded corpus of 60 random programs, checked against an independent Python
# model (its own lexer, parser, compiler, disassembler and VM); C and native identical (status, stdout, stderr) for
# every command, including scripted debugger sessions; the syntax and usage errors. Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/bcvm
N=60   # corpus programs (the model writes them)
fail=0
SECONDS=0

bad() { echo "FAIL bcvm: $*"; fail=1; }

for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { bad "unit tests ($flag)"; tail -5 "$T/t.out"; }
done
"$Y" build $D/main.yx -o "$T/bc-c" >"$T/b1.out" 2>&1 || { bad "build (c)"; cat "$T/b1.out"; }
"$Y" build $D/main.yx --native -o "$T/bc-n" >"$T/b2.out" 2>&1 || { bad "build (native)"; cat "$T/b2.out"; }
[ $fail = 0 ] || exit 1

# The independent model. It writes, for every program P: P.bc (the source), P.dis (the disassembly), P.out (stdout of
# run), P.err (stderr of run) and P.rc (its status). Nothing from the Yerux sources is used here.
python3 - "$T" "$N" <<'PY'
import os, random, sys
T, N = sys.argv[1], int(sys.argv[2])
sys.setrecursionlimit(20000)   # the 300-deep fixture nests the parser ~900 frames deep
MAX = (1 << 63) - 1
MIN = -(1 << 63)
STACK_MAX = 256
KEYWORDS = {'let', 'print', 'if', 'else'}
PREC = {'==': 1, '<': 2, '+': 3, '-': 3, '*': 4, '/': 4, '%': 4}
OPNAME = {'+': 'ADD', '-': 'SUB', '*': 'MUL', '/': 'DIV', '%': 'MOD', '<': 'LT', '==': 'EQ'}

class Bad(Exception):
    pass

def lex(src):
    toks, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c in ' \t\n\r':
            i += 1
            continue
        start = i
        if '0' <= c <= '9':
            while i < n and '0' <= src[i] <= '9':
                i += 1
            v = int(src[start:i])
            if v > MAX:
                raise Bad('integer literal too large')
            toks.append(('num', src[start:i], v, start))
        elif c.isascii() and (c.isalpha() or c == '_'):
            while i < n and src[i].isascii() and (src[i].isalnum() or src[i] == '_'):
                i += 1
            text = src[start:i]
            toks.append((text if text in KEYWORDS else 'name', text, 0, start))
        elif src.startswith('==', i):
            toks.append(('sym', '==', 0, start))
            i += 2
        elif c in '+-*/%(){};<=':
            toks.append(('sym', c, 0, start))
            i += 1
        else:
            raise Bad('unexpected byte')
    toks.append(('eof', '', 0, n))
    return toks

class Parser:
    def __init__(self, toks):
        self.t, self.p = toks, 0

    def k(self):
        return self.t[self.p][0]

    def sym(self, s):
        return self.k() == 'sym' and self.t[self.p][1] == s

    def expect(self, s):
        if not self.sym(s):
            raise Bad('expected ' + s)
        self.p += 1

    def expr(self, mn):
        lhs = self.unary()
        while self.k() == 'sym' and self.t[self.p][1] in PREC and PREC[self.t[self.p][1]] >= mn:
            op = self.t[self.p][1]
            self.p += 1
            rhs = self.expr(PREC[op] + 1)
            lhs = ('bin', op, lhs, rhs)
        return lhs

    def unary(self):
        if self.sym('-'):
            self.p += 1
            return ('neg', self.unary())
        return self.primary()

    def primary(self):
        t = self.t[self.p]
        if t[0] == 'num':
            self.p += 1
            return ('lit', t[2])
        if t[0] == 'name':
            self.p += 1
            return ('var', t[1], t[3])
        if self.sym('('):
            self.p += 1
            e = self.expr(1)
            self.expect(')')
            return e
        raise Bad('expected an expression')

    def block(self):
        self.expect('{')
        out = []
        while not self.sym('}'):
            if self.k() == 'eof':
                raise Bad('end of input in block')
            out.append(self.stmt())
        self.p += 1
        return out

    def stmt(self):
        k = self.k()
        if k == 'let':
            self.p += 1
            if self.k() != 'name':
                raise Bad('expected a name')
            name = self.t[self.p][1]
            at = self.t[self.p][3]
            self.p += 1
            self.expect('=')
            e = self.expr(1)
            self.expect(';')
            return ('let', name, at, e)
        if k == 'print':
            self.p += 1
            e = self.expr(1)
            self.expect(';')
            return ('print', e)
        if k == 'if':
            self.p += 1
            c = self.expr(1)
            then = self.block()
            els = []
            if self.k() == 'else':
                self.p += 1
                els = self.block()
            return ('if', c, then, els)
        raise Bad('expected a statement')

    def program(self):
        out = []
        while self.k() != 'eof':
            out.append(self.stmt())
        return out

def compile_program(stmts):
    # Each instruction is [mnemonic, operand]; operand None when the mnemonic has none.
    code, names, vis = [], [], []
    def lookup(name):
        for nm, slot in reversed(vis):
            if nm == name:
                return slot
        raise Bad('undefined variable')
    def expr(e):
        if e[0] == 'lit':
            code.append(['PUSH', e[1]])
        elif e[0] == 'var':
            code.append(['LOAD', lookup(e[1])])
        elif e[0] == 'neg':
            expr(e[1])
            code.append(['NEG', None])
        else:
            expr(e[2])
            expr(e[3])
            code.append([OPNAME[e[1]], None])
    def block(body):
        mark = len(vis)
        for s in body:
            stmt(s)
        del vis[mark:]
    def stmt(s):
        if s[0] == 'let':
            expr(s[3])
            slot = len(names)
            names.append(s[1])
            vis.append((s[1], slot))
            code.append(['STORE', slot])
        elif s[0] == 'print':
            expr(s[1])
            code.append(['PRINT', None])
        else:
            expr(s[1])
            jz = len(code)
            code.append(['JUMPIFZERO', 0])
            block(s[2])
            if s[3]:
                jmp = len(code)
                code.append(['JUMP', 0])
                code[jz][1] = len(code)
                block(s[3])
                code[jmp][1] = len(code)
            else:
                code[jz][1] = len(code)
    block(stmts)
    code.append(['HALT', None])
    return code, len(names)

def disasm(code):
    return ''.join('%04d %s\n' % (i, n if a is None else '%s %d' % (n, a)) for i, (n, a) in enumerate(code))

def tdiv(a, b):
    q = abs(a) // abs(b)
    return q if (a < 0) == (b < 0) else -q

def execute(code, nslots):
    out, vs, st, pc = [], [0] * nslots, [], 0
    def fault(msg):
        return ''.join(out), 1, 'bcvm: runtime error at instruction %d: %s\n' % (pc, msg)
    while True:
        name, arg = code[pc]
        if name in ('PUSH', 'LOAD'):
            if len(st) >= STACK_MAX:
                return fault('stack overflow')
            st.append(arg if name == 'PUSH' else vs[arg])
        elif name == 'STORE':
            vs[arg] = st.pop()
        elif name == 'NEG':
            a = st.pop()
            if a == MIN:
                return fault('integer overflow')
            st.append(-a)
        elif name == 'PRINT':
            out.append('%d\n' % st.pop())
        elif name == 'JUMPIFZERO':
            if st.pop() == 0:
                pc = arg
                continue
        elif name == 'JUMP':
            pc = arg
            continue
        elif name == 'HALT':
            return ''.join(out), 0, ''
        else:
            b = st.pop()
            a = st.pop()
            if name == 'LT':
                st.append(1 if a < b else 0)
            elif name == 'EQ':
                st.append(1 if a == b else 0)
            elif name in ('DIV', 'MOD'):
                if b == 0:
                    return fault('division by zero')
                q = tdiv(a, b)
                r = q if name == 'DIV' else a - b * q
                if r > MAX or r < MIN:
                    return fault('integer overflow')
                st.append(r)
            else:
                r = {'ADD': a + b, 'SUB': a - b, 'MUL': a * b}[name]
                if r > MAX or r < MIN:
                    return fault('integer overflow')
                st.append(r)
        pc += 1

def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        f.write(text)

def emit(base, src):
    write(base + '.bc', src)
    code, nslots = compile_program(Parser(lex(src)).program())
    out, rc, err = execute(code, nslots)
    write(base + '.dis', disasm(code))
    write(base + '.out', out)
    write(base + '.err', err)
    write(base + '.rc', '%d\n' % rc)

def nested(depth):
    return 'let r = ' + '1 - (' * depth + '1' + ')' * depth + ';\nprint r;\n'

FIXED = {
    'arith': """let a = 7;
let b = -a * 3 + (10 % 4) / 2;
print b;
print a == 7;
print a < b;
if a < 10 { print 1; let z = 5; print z; } else { print 2; }
print -(-3) - 1;
""",
    'precedence': "print 2 + 3 * 4 - -1;\nprint 2 * 3 % 4;\nprint 100 - 30 - 5;\nprint -7 / 2;\n"
                  "print -7 % 3;\nprint 7 % -3;\nprint 1 + 1 == 2;\nprint 2 < 3 == 1;\nprint 4 == 4 == 1;\nprint 3 < 2;\n",
    'shadow': "let x = 1;\nlet x = x + 10;\nprint x;\nif x < 0 { let x = 0; print x; }\nprint x;\n",
    'nesting': "let n = 5;\nif n < 3 { print 0; } else { if n < 10 { print 1; } else { print 2; } }\n"
               "if 0 { print 9; }\nif 1 { let q = 1; if q { print 5; } }\nprint n;\n",
    'empty': "",
    'minint': "let m = -9223372036854775807 - 1;\nprint m;\nprint m + 0;\nprint 9223372036854775807 * 1;\n"
              "print -(m + 1);\n",
    'divzero': "print 1;\nprint 5 / (3 - 3);\nprint 2;\n",
    'overflow': "let m = 9223372036854775807;\nprint m;\nprint m + 1;\n",
    'mulover': "print 3037000500 * 3037000500;\n",
    'modmin': "let m = -9223372036854775807 - 1;\nprint m % -1;\nprint m / -1;\n",
    'deep200': nested(200),
    'deep300': nested(300),
    'negs': "print - - - 5;\nprint -(2 * -3);\n",
}

# Syntax and name errors: the exact message of each (the path is the file's).
SYNTAX = {
    'sy_plus': ("print 1 +;", "byte 9: expected an expression but found ';'"),
    'sy_big': ("let x = 9223372036854775808;", "byte 8: integer literal too large"),
    'sy_hash': ("let x = 3 # 4;", "byte 10: unexpected byte 35"),
    'sy_undef': ("let r = q;", "byte 8: undefined variable 'q'"),
    'sy_eof_block': ("if 1 { print 2;", "byte 15: expected '}' but found end of input"),
    'sy_semi': ("print 1", "byte 7: expected ';' but found end of input"),
    'sy_name': ("let = 1;", "byte 4: expected a name but found '='"),
    'sy_gt': ("print 2 > 1;", "byte 8: unexpected byte 62"),
    'sy_else': ("else { }", "byte 0: expected a statement but found 'else'"),
}

def gen_program(rng):
    NAMES = ['a', 'b', 'c', 'x1', 'y_2', 'val']
    def ws():
        return rng.choice(['', ' ', ' ', '\n', '\t', '  '])
    def atom(vis):
        if vis and rng.random() < 0.45:
            return rng.choice(vis)
        r = rng.random()
        if r < 0.05:
            return '0'
        if r < 0.08:
            return '9223372036854775807'
        if r < 0.11:
            return '3037000499'
        return str(rng.randrange(1, 30))
    def expr(d, vis):
        r = rng.random()
        if d >= 3 or r < 0.3:
            return atom(vis)
        if r < 0.4:
            return '-' + ws() + expr(d + 1, vis)
        if r < 0.5:
            return '(' + ws() + expr(d + 1, vis) + ws() + ')'
        op = rng.choice(['+', '-', '*', '/', '%', '<', '=='])
        return expr(d + 1, vis) + ws() + op + ws() + expr(d + 1, vis)
    def block(d, vis):
        parts = []
        for _ in range(rng.randrange(0, 3)):
            parts.append(stmt(d, vis))
        return ' '.join(parts)
    def stmt(d, vis):
        r = rng.random()
        if r < 0.3:
            name = rng.choice(NAMES)
            text = 'let ' + name + ' = ' + expr(0, vis) + ';'
            vis.append(name)
            return text
        if r < 0.7 or d >= 2:
            return 'print ' + expr(0, vis) + ';'
        text = 'if ' + expr(0, vis) + ' {' + block(d + 1, list(vis)) + '}'
        if rng.random() < 0.5:
            text += ' else {' + block(d + 1, list(vis)) + '}'
        return text
    vis = []
    return rng.choice(['\n', ' ']).join(stmt(0, vis) for _ in range(rng.randrange(1, 10))) + '\n'

for name, src in FIXED.items():
    emit(os.path.join(T, 'fx', name), src)
for name, (src, msg) in SYNTAX.items():
    base = os.path.join(T, 'fx', name)
    write(base + '.bc', src)
    write(base + '.err', 'bcvm: %s: %s\n' % (base + '.bc', msg))
    write(base + '.rc', '2\n')

rng = random.Random(20260)
for i in range(N):
    emit(os.path.join(T, 'corpus', 'c%02d' % i), gen_program(rng))
PY
[ $? = 0 ] || { bad "python model"; exit 1; }

# ---- commands: C and native must agree on status, stdout and stderr ----
run_both() { # stdin-file, then the arguments; leaves the C outputs in $T/out.c, $T/err.c, $T/st.c
  local in=$1; shift
  for b in c n; do
    "$T/bc-$b" "$@" <"$in" >"$T/out.$b" 2>"$T/err.$b"; echo $? >"$T/st.$b"
  done
  cmp -s "$T/out.c" "$T/out.n" && cmp -s "$T/err.c" "$T/err.n" && cmp -s "$T/st.c" "$T/st.n" || { bad "C and native differ: $*"; return 1; }
  return 0
}

# The disassembly and the run of one program against the model's files.
check_prog() { # base-path (without .bc), label
  local base=$1
  run_both /dev/null dis "$base.bc" && { cmp -s "$base.dis" "$T/out.c" || bad "$2: dis differs from the model"; }
  run_both /dev/null run "$base.bc"
  cmp -s "$base.out" "$T/out.c" || { bad "$2: stdout differs from the model"; diff "$base.out" "$T/out.c" | head -4; }
  cmp -s "$base.err" "$T/err.c" || { bad "$2: stderr differs from the model"; head -2 "$T/err.c"; }
  [ "$(cat "$T/st.c")" = "$(cat "$base.rc")" ] || bad "$2: status $(cat "$T/st.c"), model $(cat "$base.rc")"
}

for f in "$T"/fx/*.bc; do
  case "$(basename "$f")" in sy_*) continue ;; esac
  check_prog "${f%.bc}" "fixed $(basename "${f%.bc}")"
done

for f in "$T"/fx/sy_*.bc; do
  base=${f%.bc}
  run_both /dev/null run "$f"
  cmp -s "$base.err" "$T/err.c" || { bad "syntax error message ($(basename "$base")): $(cat "$T/err.c")"; }
  [ "$(cat "$T/st.c")" = 2 ] && [ ! -s "$T/out.c" ] || bad "syntax error status ($(basename "$base")): $(cat "$T/st.c")"
  run_both /dev/null dis "$f"
  [ "$(cat "$T/st.c")" = 2 ] && cmp -s "$base.err" "$T/err.c" || bad "dis of a syntax error ($(basename "$base"))"
done

corpus=0
for f in "$T"/corpus/c*.bc; do
  corpus=$((corpus + 1))
  check_prog "${f%.bc}" "corpus $(basename "${f%.bc}")"
done
[ $corpus = "$N" ] || bad "corpus has $corpus programs, want $N"

# Usage and I/O errors: status 2, the message on standard error.
usage_msg='bcvm: usage: bcvm run|dis|debug FILE'
run_both /dev/null; [ "$(cat "$T/st.c")" = 2 ] && [ "$(cat "$T/err.c")" = "$usage_msg" ] || bad "no arguments: $(cat "$T/st.c") $(cat "$T/err.c")"
run_both /dev/null frob "$T/fx/arith.bc"; [ "$(cat "$T/st.c")" = 2 ] && [ "$(cat "$T/err.c")" = "$usage_msg" ] || bad "unknown command: $(cat "$T/st.c") $(cat "$T/err.c")"
run_both /dev/null run "$T/fx/arith.bc" extra; [ "$(cat "$T/st.c")" = 2 ] && [ "$(cat "$T/err.c")" = "$usage_msg" ] || bad "extra argument: $(cat "$T/st.c")"
run_both /dev/null run "$T/none.bc"; [ "$(cat "$T/st.c")" = 2 ] && grep -q "^bcvm: cannot read $T/none.bc: " "$T/err.c" || bad "missing file: $(cat "$T/st.c") $(cat "$T/err.c")"

# ---- the debugger, with scripted stdin ----
printf 's\ns\np\nb 5\nc\np\nc\nc\nd\nq\n' >"$T/dbg1.in"
run_both "$T/dbg1.in" debug "$T/fx/arith.bc"; [ "$(cat "$T/st.c")" = 0 ] || bad "debug session 1 status $(cat "$T/st.c")"
for want in "> 0000 PUSH 7" "breakpoint set at 5" "breakpoint at 5" "stack: -7 3" "vars: a=7 b=0 z=0" "halted" \
            "program has finished" "* 0005 MUL" "> 0042 HALT" "-20"; do
  grep -qF -- "$want" "$T/out.c" || bad "debug session 1: no line '$want'"
done

printf 'c\n' >"$T/dbg2.in"
run_both "$T/dbg2.in" debug "$T/fx/divzero.bc"; [ "$(cat "$T/st.c")" = 1 ] || bad "debug runtime error status $(cat "$T/st.c")"
[ "$(cat "$T/err.c")" = "bcvm: runtime error at instruction 6: division by zero" ] || bad "debug runtime error message: $(cat "$T/err.c")"
grep -qxF -- "> 0006 DIV" "$T/out.c" || bad "debug runtime error: the faulting instruction is not traced"

: >"$T/dbg3.in"
run_both "$T/dbg3.in" debug "$T/fx/arith.bc"
[ "$(cat "$T/out.c")" = "(bcvm) " ] && [ "$(cat "$T/st.c")" = 0 ] || bad "debug at end of input"

printf 'zz\nb x\nb 99\nb\nh\nq\n' >"$T/dbg4.in"
run_both "$T/dbg4.in" debug "$T/fx/arith.bc"
for want in "unknown command 'zz'; h lists the commands" "usage: b N" "no instruction 99"; do
  grep -qF -- "$want" "$T/out.c" || bad "debug bad commands: no line '$want'"
done
[ "$(grep -c -F -- "usage: b N" "$T/out.c")" = 2 ] || bad "debug: 'b x' and 'b' should both print the usage"

printf 'p\nc\nc\nq\n' >"$T/dbg5.in"
run_both "$T/dbg5.in" debug "$T/fx/empty.bc"
[ "$(cat "$T/out.c")" = "$(printf '(bcvm) stack: (empty)\nvars: (none)\n(bcvm) > 0000 HALT\nhalted\n(bcvm) program has finished\n(bcvm) ')" ] \
  || bad "debug on an empty program: $(cat "$T/out.c")"

fixed=$(ls "$T"/fx/*.bc | grep -vc '/sy_')
[ $fail = 0 ] && echo "bcvm: ok (unit tests on both backends; $fixed fixed and $corpus corpus programs match the model; syntax errors and debugger sessions checked; C and native identical; ${SECONDS}s)"
exit $fail
