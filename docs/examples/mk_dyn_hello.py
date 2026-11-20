# Hand-assembles a dynamically linked x86-64 ELF that calls libc write() and exit().
# Usage: python3 mk_dyn_hello.py out && chmod +x out && ./out   (verified: prints "hello", exit 0, glibc)
# Mirrors docs/FFI.md section 9. Offsets are hardcoded to that layout.
import struct,sys
BASE=0x400000
interp=b"/lib64/ld-linux-x86-64.so.2\0"
strtab=b"\0libc.so.6\0write\0exit\0"
o_libc=1;o_write=11;o_exit=17
msg=b"hello\n"
# layout in one file, two PT_LOADs: seg0 R+X (0..0x1000) , seg1 RW (0x1000..)
EH=64;PH=56;NPH=4
off=EH+PH*NPH
o_interp=off; off+=len(interp)
off=(off+7)&~7
o_sym=off; off+=24*3
o_str=off; off+=len(strtab)
off=(off+7)&~7
o_rela=off; off+=24*2
o_msg=off; off+=len(msg)
o_code=off
code_len=0x40
off+=code_len
assert off<0x1000
# RW seg at file offset 0x1000, vaddr BASE+0x1000
o_dyn=0x1000; ndyn=9
o_got=o_dyn+16*ndyn
va=lambda x:BASE+x
got_write=va(o_got); got_exit=va(o_got+8)
# code
c=bytearray()
c+=b"\xbf"+struct.pack("<I",1)                  # mov edi,1
c+=b"\x48\xbe"+struct.pack("<Q",va(o_msg))      # movabs rsi,msg
c+=b"\xba"+struct.pack("<I",len(msg))           # mov edx,6
def callrip(got):
    here=va(o_code)+len(c)+6
    return b"\xff\x15"+struct.pack("<i",got-here)
c+=callrip(got_write)
c+=b"\x31\xff"                                  # xor edi,edi
c+=callrip(got_exit)
c+=b"\xf4"                                      # hlt (unreachable)
assert len(c)<=code_len
c+=b"\x90"*(code_len-len(c))
f=bytearray(0x2000)
# ehdr
f[0:16]=b"\x7fELF\2\1\1\0"+b"\0"*8
struct.pack_into("<HHIQQQIHHHHHH",f,16,2,62,1,va(o_code),EH,0,0,EH,PH,NPH,0,0,0)
def ph(i,t,fl,o,v,fs,ms,al):
    struct.pack_into("<IIQQQQQQ",f,EH+PH*i,t,fl,o,v,v,fs,ms,al)
ph(0,3,4,o_interp,va(o_interp),len(interp),len(interp),1)           # PT_INTERP
ph(1,1,5,0,BASE,0x1000,0x1000,0x1000)                               # PT_LOAD R+X
ph(2,1,6,0x1000,va(0x1000),o_got+16-0x1000,o_got+16-0x1000,0x1000)                      # PT_LOAD RW
ph(3,2,6,o_dyn,va(o_dyn),16*ndyn,16*ndyn,8)                         # PT_DYNAMIC
f[o_interp:o_interp+len(interp)]=interp
# symtab: null, write, exit  (Elf64_Sym: name u32, info u8, other u8, shndx u16, value u64, size u64)
struct.pack_into("<IBBHQQ",f,o_sym+24,o_write,0x12,0,0,0,0)   # GLOBAL FUNC UND
struct.pack_into("<IBBHQQ",f,o_sym+48,o_exit,0x12,0,0,0,0)
f[o_str:o_str+len(strtab)]=strtab
# rela: offset, info=(sym<<32)|R_X86_64_GLOB_DAT(6), addend
struct.pack_into("<QQq",f,o_rela,got_write,(1<<32)|6,0)
struct.pack_into("<QQq",f,o_rela+24,got_exit,(2<<32)|6,0)
f[o_msg:o_msg+len(msg)]=msg
f[o_code:o_code+code_len]=c
dyn=[(1,o_libc),(5,va(o_str)),(6,va(o_sym)),(10,len(strtab)),(11,24),(7,va(o_rela)),(8,48),(9,24),(0,0)]
for i,(t,v) in enumerate(dyn): struct.pack_into("<QQ",f,o_dyn+16*i,t,v)
open(sys.argv[1] if len(sys.argv)>1 else "hello_dyn","wb").write(f[:o_got+16])

