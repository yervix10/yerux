import os,sys,random,subprocess,glob,hashlib,concurrent.futures as cf
ROOT=os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
Y=os.environ.get('YERUX',ROOT+'/target/release/yerux')
seed=int(sys.argv[1]); n=int(sys.argv[2]); out=sys.argv[3]
os.makedirs(out,exist_ok=True)
files=[f for f in sorted(glob.glob(ROOT+'/tests/*.yx'))+sorted(glob.glob(ROOT+'/examples/*.yx'))+sorted(glob.glob(ROOT+'/examples/real/*.yx'))+sorted(glob.glob(ROOT+'/examples/real/*/*.yx')) if os.path.getsize(f)<60000]
rnd=random.Random(seed)
WEIRD=['‮','﻿','é','日本語','😀','\u0000','\x01','\x7f','​','퟿','\U0010ffff','\r','\t','\x0b']
BIG=['99999999999999999999999999','9223372036854775808','-9223372036854775809','1e999999','0x','0xFFFFFFFFFFFFFFFFFF','1_000_000_000_000_000_000_000','340282366920938463463374607431768211456','0.0000000000000000000000000001e-400']
def tokens(s):
    import re
    return re.findall(r'\s+|\w+|[^\w\s]',s)
def mutate(s):
    k=rnd.choice(['ident','ident','ident','kw','kw','num','typ','trunc','dup','delbrace','weird','big','deltok','swaptok','insbrace','repeat','nest','delline','dupline','junk'])
    if k=='trunc': return s[:rnd.randrange(0,len(s)+1)]
    t=tokens(s)
    if not t: return s
    i=rnd.randrange(len(t))
    if k in ('ident','kw','num','typ'):
        ids=[x for x,y in enumerate(t) if y.isidentifier()]
        if k=='ident' and len(ids)>1:
            a,b=rnd.sample(ids,2); t[a]=t[b]
        elif k=='kw' and ids:
            t[rnd.choice(ids)]=rnd.choice(['view','mut','move','let','var','fn','match','if','while','for','return','struct','enum','impl','trait','self','Self','Box','List','Option','Result','String','Int','Bool','Text','Float','Map','Shared','dyn','spawn','scope','unsafe','comptime','defer','as','else','in','true','false','_','pub','import'])
        elif k=='num':
            nums=[x for x,y in enumerate(t) if y.isdigit()]
            if nums: t[rnd.choice(nums)]=rnd.choice(['0','1','-1','2','255','256','65536','4294967296','9223372036854775807'])
        elif k=='typ':
            tys=[x for x,y in enumerate(t) if y in ('Int','String','Bool','Float','Text','List','Option','Char','UInt8')]
            if tys: t[rnd.choice(tys)]=rnd.choice(['Int','String','Bool','Float','Text','List','Option','Char','UInt8','Box','Int<0..3>','[Int; 3]','(Int, Int)','fn(Int) -> Int','dyn Show'])
        return ''.join(t)
    if k=='dup': t.insert(i,t[i]); return ''.join(t)
    if k=='deltok': del t[i]; return ''.join(t)
    if k=='swaptok':
        j=rnd.randrange(len(t)); t[i],t[j]=t[j],t[i]; return ''.join(t)
    if k=='delbrace':
        idx=[x for x,y in enumerate(t) if y in '{}()[]']
        if idx: del t[rnd.choice(idx)]
        return ''.join(t)
    if k=='insbrace': t.insert(i,rnd.choice(['{','}','(',')','[',']','"',"'",'`','/*','//','\\']))
    elif k=='weird': t.insert(i,rnd.choice(WEIRD)*rnd.choice([1,1,3,100]))
    elif k=='big':
        nums=[x for x,y in enumerate(t) if y.isdigit()]
        if nums: i=rnd.choice(nums)
        t[i]=rnd.choice(BIG)
    elif k=='repeat': t.insert(i,rnd.choice(['(','[','{','-','!','&','*','not '])*rnd.choice([50,500,5000]))
    elif k=='nest':
        t.insert(i,rnd.choice(['(','[','{'])*rnd.choice([100,2000,20000]))
    elif k=='delline':
        L=s.split('\n'); 
        if L: del L[rnd.randrange(len(L))]
        return '\n'.join(L)
    elif k=='dupline':
        L=s.split('\n'); j=rnd.randrange(len(L)); L.insert(j,L[j]); return '\n'.join(L)
    elif k=='junk': return s+''.join(rnd.choice(WEIRD+list('{}()[]<>"\'`\\/*+-=')) for _ in range(rnd.randrange(1,80)))
    return ''.join(t)
def run(case):
    idx,path,src=case
    p=os.path.join(out,'c%d.yx'%idx)
    open(p,'w',encoding='utf-8',errors='surrogatepass').write(src) if False else open(p,'wb').write(src.encode('utf-8','surrogatepass') if True else b'')
    try:
        r=subprocess.run([Y,'check',p],capture_output=True,timeout=5)
    except subprocess.TimeoutExpired:
        return (idx,path,'HANG',b'')
    err=r.stderr+r.stdout
    if r.returncode<0 or r.returncode>=101 or b'panicked at src/' in err or b'RUST_BACKTRACE' in err or b'has overflowed its stack' in err:
        return (idx,path,'PANIC rc=%d'%r.returncode,err[-600:])
    if r.returncode!=0 and not err.strip():
        return (idx,path,'SILENT rc=%d'%r.returncode,b'')
    if os.environ.get('DEEP'):
        try: rf=subprocess.run([Y,'fmt','--stdout',p],capture_output=True,timeout=10)
        except subprocess.TimeoutExpired: return (idx,path,'HANG fmt',b'')
        if rf.returncode<0 or rf.returncode>=101 or b'panicked at src/' in rf.stderr: return (idx,path,'PANIC fmt',rf.stderr[-600:])
        if rf.returncode==0 and r.returncode==0:
            q=p+'.fmt.yx'; open(q,'wb').write(rf.stdout)
            r2=subprocess.run([Y,'check',q],capture_output=True,timeout=10)
            if r2.returncode!=0: return (idx,path,'FMT output no longer checks',r2.stderr[-400:])
            rf2=subprocess.run([Y,'fmt','--stdout',q],capture_output=True,timeout=10)
            if rf2.stdout!=rf.stdout: return (idx,path,'FMT not idempotent',b'')
            os.remove(q)
    if r.returncode==0 and os.environ.get('DEEP'):
        for sub in (['c',p],['ir',p]):
            try: r2=subprocess.run([Y]+sub,capture_output=True,timeout=10)
            except subprocess.TimeoutExpired: return (idx,path,'HANG '+sub[0],b'')
            e2=r2.stderr
            if r2.returncode<0 or r2.returncode>=101 or b'panicked at src/' in e2: return (idx,path,'PANIC '+sub[0]+' rc=%d'%r2.returncode,e2[-600:])
    os.remove(p)
    return None
cases=[]
for i in range(n):
    f=rnd.choice(files)
    s=open(f,encoding='utf-8',errors='replace').read()
    for _ in range(rnd.choice([1,1,2,3])): s=mutate(s)
    cases.append((seed*1000000+i,f,s))
bad=[]
with cf.ThreadPoolExecutor(6) as ex:
    for r in ex.map(run,cases):
        if r: bad.append(r); print(r[0],os.path.basename(r[1]),r[2],r[3][-300:].decode('utf-8','replace').replace('\n',' | ')[:300],flush=True)
print('done',n,'bad',len(bad))
