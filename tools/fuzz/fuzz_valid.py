import os,sys,random,subprocess,glob,re,shutil,concurrent.futures as cf
ROOT=os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
Y=os.environ.get('YERUX',ROOT+'/target/release/yerux')
seed=int(sys.argv[1]); n=int(sys.argv[2]); out=sys.argv[3]; ASAN=os.environ.get('ASAN')
os.makedirs(out,exist_ok=True)
BAD=re.compile(r'import std\.(net|process|fs|env|time|rand|http|io)|spawn|parallel|Atomic|scope|read_line|stdin|thread|Mutex|Shared|Locked|Sender|std\.http')
files=[]
for f in sorted(glob.glob(ROOT+'/tests/*.yx'))+sorted(glob.glob(ROOT+'/examples/*.yx')):
    b=os.path.basename(f)
    if re.match(r'e\d',b) or os.path.getsize(f)>12000: continue
    s=open(f,encoding='utf-8',errors='replace').read()
    if BAD.search(s) or 'fn main' not in s or 'import ' in s: continue
    files.append((f,s))
rnd=random.Random(seed)
INTS=['0','1','2','-1','3','7','10','255','256','1000','65535','2147483647','4294967296','9223372036854775807','-9223372036854775807']
ARITH=['+','-','*','/','%']; CMP=['<','>','<=','>=','==','!=']; LOG=['&&','||']
TYPES=['Int','Float','Bool','String','Char','UInt8','UInt64','Int32','Int8','UInt16','Int16','UInt32','Float32']
STR=re.compile(r'"(?:[^"\\\n]|\\.)*"')
def lit_mut(s,k):
    toks=list(re.finditer(r'(?<![\w.])\d+(?:\.\d+)?(?![\w.])',s))
    if not toks: return s
    m=rnd.choice(toks); r=rnd.choice(INTS) if '.' not in m.group() else rnd.choice(['0.0','1.5','-1.0','1e300','0.1','3.14159','1e-300'])
    return s[:m.start()]+r+s[m.end():]
def op_mut(s,k):
    pat={'arith':r' ([-+*/%]) ','cmp':r' (<=|>=|==|!=|<|>) ','log':r' (&&|\|\|) '}[k]
    ms=list(re.finditer(pat,s))
    if not ms: return s
    m=rnd.choice(ms); pool={'arith':ARITH,'cmp':CMP,'log':LOG}[k]
    return s[:m.start(1)]+rnd.choice(pool)+s[m.end(1):]
def type_mut(s,k):
    ms=list(re.finditer(r'\b(Int|Float|Bool|String|Char|UInt8|UInt64|Int32)\b',s))
    if not ms: return s
    m=rnd.choice(ms)
    return s[:m.start()]+rnd.choice(TYPES)+s[m.end():]
def bool_mut(s,k):
    ms=list(re.finditer(r'\b(true|false)\b',s))
    if not ms: return s
    m=rnd.choice(ms); return s[:m.start()]+('false' if m.group()=='true' else 'true')+s[m.end():]
def str_mut(s,k):
    ms=list(STR.finditer(s))
    if not ms: return s
    m=rnd.choice(ms); return s[:m.start()]+rnd.choice(['""','"x"','"é日本😀"','"{0}"'.replace('{0}','a'),'"'+'a'*300+'"'])+s[m.end():]
def stmt_swap(s,k):
    L=s.split('\n')
    idx=[i for i,l in enumerate(L) if re.match(r'\s+(let|var|print|[a-z_]\w*[ .(\[]).*[^{,(]$',l)]
    if len(idx)<2: return s
    i=rnd.choice(idx[:-1]); 
    j=i+1
    if j in idx: L[i],L[j]=L[j],L[i]
    return '\n'.join(L)
def dup_fn(s,k):
    m=re.search(r'^fn (\w+)(<[^>]*>)?\((.*?)\n}\n',s,re.M|re.S)
    if not m or m.group(1)=='main': return s
    body=m.group(0).replace('fn '+m.group(1),'fn '+m.group(1)+'_copy',1)
    return s[:m.end()]+'\n'+body+s[m.end():]
def mode_mut(s,k):
    ms=list(re.finditer(r'\b(view|mut|move)\b ?',s))
    if not ms: return s
    m=rnd.choice(ms); return s[:m.start()]+rnd.choice(['','view ','mut ','move '])+s[m.end():]
def wrap_closure(s,k):
    ms=list(re.finditer(r'print\((.*)\)\n',s))
    if not ms: return s
    m=rnd.choice(ms)
    return s[:m.start()]+'let wrapped = || { print(%s) }\n'%m.group(1)+'wrapped()\n'+s[m.end():]
def generic_id(s,k):
    return s+'\nfn ident_g<T>(x: T) -> T { return x }\n'
def dup_stmt(s,k):
    L=s.split('\n'); idx=[i for i,l in enumerate(L) if re.match(r'\s+print\(',l)]
    if not idx: return s
    i=rnd.choice(idx); L.insert(i,L[i]); return '\n'.join(L)
def neg(s,k):
    ms=list(re.finditer(r'\b[a-z_]\w*\b(?=[ )\n,])',s))
    return s
M=[lambda s:lit_mut(s,0)]*4+[lambda s:op_mut(s,'arith')]*3+[lambda s:op_mut(s,'cmp')]*2+[lambda s:op_mut(s,'log')]+[lambda s:type_mut(s,0)]*2+[lambda s:bool_mut(s,0),lambda s:str_mut(s,0),lambda s:stmt_swap(s,0),lambda s:dup_fn(s,0),lambda s:mode_mut(s,0),lambda s:wrap_closure(s,0),lambda s:generic_id(s,0),lambda s:dup_stmt(s,0)]
def run(case):
    idx,path,src=case
    d=os.path.join(out,'c%d'%idx); os.makedirs(d,exist_ok=True)
    p=d+'/p.yx'; open(p,'w',encoding='utf-8').write(src)
    def sh(cmd,t=10):
        try:
            r=subprocess.run(cmd,capture_output=True,timeout=t,cwd=d); return r
        except subprocess.TimeoutExpired: return None
    def bad(kind,detail=b''):
        return (idx,os.path.basename(path),kind,detail[-500:])
    r=sh([Y,'check',p],5)
    if r is None: return bad('HANG check')
    e=r.stderr+r.stdout
    if r.returncode<0 or r.returncode>=101 or b'panicked at src/' in e: return bad('PANIC check',e)
    if r.returncode!=0:
        shutil.rmtree(d); return None
    open(out+'/stat_ok','a').write('1')
    for sub in ('c','ir'):
        r=sh([Y,sub,p],20)
        if r is None: return bad('HANG '+sub)
        if r.returncode!=0 or b'panicked at src/' in r.stderr: return bad('FAIL '+sub+' after ok check',r.stderr)
    rc=sh([Y,'build',p,'-o',d+'/cexe'],60)
    if rc is None: return bad('HANG build C')
    if rc.returncode!=0: return bad('FAIL build C',rc.stderr+rc.stdout)
    rn=sh([Y,'build',p,'--native','-o',d+'/nexe'],60)
    if rn is None: return bad('HANG build native')
    if rn.returncode!=0:
        if b'native backend' in rn.stderr+rn.stdout and b'not supported' in rn.stderr+rn.stdout: shutil.rmtree(d); return None
        return bad('FAIL build native',rn.stderr+rn.stdout)
    open(out+'/stat_ran','a').write('1')
    a=sh([d+'/cexe'],5); b=sh([d+'/nexe'],5)
    if a is None or b is None:
        if (a is None)!=(b is None): return bad('HANG differs C=%s native=%s'%(a is None,b is None))
        shutil.rmtree(d); return None
    # panics (stderr text) may carry positions; compare stdout and exit code
    if a.returncode==-11 and b.returncode==101 and b'stack overflow' in b.stderr:
        shutil.rmtree(d); return None  # documented: the C backend has no stack guard (SPEC.md)
    if a.stdout!=b.stdout or a.returncode!=b.returncode:
        return bad('DIFF C vs native rc %d/%d'%(a.returncode,b.returncode),b'C:'+a.stdout[-200:]+b' N:'+b.stdout[-200:])
    if a.returncode<0 or b.returncode<0: return bad('SIGNAL rc %d/%d'%(a.returncode,b.returncode),a.stderr+b.stderr)
    if ASAN:
        env=dict(os.environ,YERUX_CC_FLAGS='-fsanitize=address,undefined -fno-sanitize-recover=undefined -g')
        try:
            r3=subprocess.run([Y,'build',p,'-o',d+'/aexe'],capture_output=True,timeout=90,cwd=d,env=env)
            if r3.returncode==0:
                r4=subprocess.run([d+'/aexe'],capture_output=True,timeout=20,cwd=d,env=dict(os.environ,ASAN_OPTIONS='detect_leaks=1'))
                if b'ERROR: AddressSanitizer' in r4.stderr or b'runtime error' in r4.stderr or (b'LeakSanitizer' in r4.stderr and r4.returncode != 101): return bad('ASAN',r4.stderr)
        except subprocess.TimeoutExpired: return bad('HANG asan')
    shutil.rmtree(d); return None
cases=[]
for i in range(n):
    f,s=rnd.choice(files)
    for _ in range(rnd.choice([1,1,2,3])): s=rnd.choice(M)(s)
    cases.append((seed*1000000+i,f,s))
bad=[]
with cf.ThreadPoolExecutor(int(os.environ.get('J','6'))) as ex:
    for r in ex.map(run,cases):
        if r: bad.append(r); print(r[0],r[1],r[2],r[3].decode('utf-8','replace').replace('\n',' | ')[:400],flush=True)
sz=lambda f: os.path.getsize(out+'/'+f) if os.path.exists(out+'/'+f) else 0
print('done',n,'files',len(files),'check-ok',sz('stat_ok'),'ran',sz('stat_ran'),'bad',len(bad))
