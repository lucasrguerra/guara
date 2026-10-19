#!/usr/bin/env python3
"""Gera a logo do Guará em estilo geométrico facetado (low-poly).

A cabeça do lobo-guará foi contornada sobre uma foto de referência, com as
regiões de cor do animal real: focinho escuro subindo até entre os olhos, juba
escura na nuca, interior das orelhas e garganta claros. As facetas (Delaunay)
seguem essas regiões, e o pescoço se solta em estilhaços para a esquerda.

Uso: python3 scripts/make-logo.py recorte.png [saida.svg]
Requer numpy, scipy e pillow. O ícone (guara.svg) embute esta mesma arte.
"""
import math, random, sys
import numpy as np
from PIL import Image
from scipy.spatial import Delaunay
OUT=[(98,382),(104,350),(110,318),(114,290),(116,262),(110,235),(100,212),
(84,178),(66,140),(54,100),(50,68),(54,44),(70,52),(98,70),(128,94),(156,118),(176,146),
(180,112),(190,78),(204,50),(216,32),(230,40),(252,64),(274,94),(292,120),(305,135),
(330,138),(370,142),(420,150),(500,165),(500,460),(320,460),(300,420),(260,385),(225,372),(190,380),(150,398),(118,404),(102,396)]
EARS=[[(52,78),(80,96),(115,118),(150,142),(130,170),(105,185),(85,160),(65,120)],
      [(222,48),(245,62),(270,95),(285,125),(255,165),(215,165),(195,130),(200,85)]]
EYES=[(125,270),(200,268)]
ZONES=[
 # interior claro das orelhas (com borda verde em volta)
 ('pale',[(62,62),(92,78),(122,100),(150,124),(160,144),(138,166),(116,190),(98,166),(76,124),(62,92)]),
 ('pale',[(214,50),(228,56),(248,78),(268,106),(282,128),(258,150),(226,160),(200,146),(196,112),(204,76)]),
 # testa um pouco mais clara entre as orelhas
 ('brow',[(160,150),(185,148),(240,170),(270,190),(250,220),(200,230),(160,225),(130,205)]),
 # área dos olhos, um tom mais escuro
 ('mask',[(108,250),(140,250),(165,246),(195,250),(222,262),(220,284),(196,292),(165,282),(140,290),(112,290)]),
 # juba escura na nuca
 ('mane',[(298,136),(500,166),(500,236),(430,214),(360,198),(318,182),(300,160)]),
 # garganta clara e linha clara do lábio
 ('pale',[(214,350),(234,332),(266,324),(302,320),(316,344),(306,372),(274,388),(240,380),(220,370)]),
 ('pale',[(150,398),(186,382),(212,368),(222,374),(204,388),(166,404)]),
 # focinho preto subindo pelo dorso do nariz até entre os olhos
 ('muzzle',[(96,384),(102,352),(108,322),(114,296),(132,284),(150,266),(164,258),(176,268),(184,294),(196,322),(206,352),(200,374),(170,392),(130,404),(104,398)]),
]

from PIL import ImageFilter
REF = sys.argv[1]  # recorte da cabeça da foto de referência (390x460), não versionado
img=np.array(Image.open(REF).convert('RGB').filter(ImageFilter.MedianFilter(9)).filter(ImageFilter.GaussianBlur(4))).astype(float)
Hc,Wc=img.shape[:2]
rnd=random.Random(7)
# contorno: estende o pescoço para a esquerda (depois do espelhamento)
C=[p for p in OUT if p[0]<=500]
def inside(poly,x,y):
    c=False; j=len(poly)-1
    for i in range(len(poly)):
        xi,yi=poly[i]; xj,yj=poly[j]
        if (yi>y)!=(yj>y) and x<(xj-xi)*(y-yi)/(yj-yi)+xi: c=not c
        j=i
    return c
# pontos: contorno reamostrado + traços anatômicos + preenchimento com jitter
pts=[]
for i in range(len(C)):
    a=C[i]; b=C[(i+1)%len(C)]; n=max(1,int(math.dist(a,b)/16))
    for k in range(n): pts.append((a[0]+(b[0]-a[0])*k/n, a[1]+(b[1]-a[1])*k/n))
FEAT=[(125,270),(200,268),(110,300),(160,286),(185,300),(100,383),(118,404),(150,396),(200,360),
      (215,330),(260,322),(300,318),(310,350),(270,386),(140,140),(110,190),(80,120),(255,100),(270,150),(225,160),
      (205,110),(175,180),(230,230),(150,230),(300,200),(340,250),(320,170),(360,330),(400,260),(450,330),(260,280)]
pts+=FEAT
# vértices nas bordas das regiões: as facetas seguem os contornos das cores
for k_,Z in ZONES:
    for i in range(len(Z)):
        a=Z[i]; b=Z[(i+1)%len(Z)]; n=max(1,int(math.dist(a,b)/18))
        for j in range(n):
            q=(a[0]+(b[0]-a[0])*j/n, a[1]+(b[1]-a[1])*j/n)
            if inside(C,*q) and min(math.dist(q,p_) for p_ in pts)>6: pts.append(q)
sp=24
for y in np.arange(20,470,sp):
    for x in np.arange(20,510,sp):
        jx,jy=x+rnd.uniform(-8,8),y+rnd.uniform(-8,8)
        if inside(C,jx,jy) and min(math.dist((jx,jy),p) for p in pts)>15: pts.append((jx,jy))
P=np.array(pts)
tri=Delaunay(P)
def sample(t):
    a,b,c=P[t]
    cols=[]
    for _ in range(40):
        u,v=rnd.random(),rnd.random()
        if u+v>1: u,v=1-u,1-v
        x,y=a+(b-a)*u+(c-a)*v
        x=min(max(x,0),Wc-1); y=min(max(y,0),Hc-1)
        if x>380: x=380-(x-380)*0.2  # o pescoço além da foto herda a cor da borda
        cols.append(img[int(y),int(x)])
    m=np.mean(cols,axis=0); return m
RAMP=["#083a45","#0f766e","#14b8a6","#5eead4","#c9faf1"]
def tone(rgb):
    r,g,b=rgb; L=(0.299*r+0.587*g+0.114*b)/255
    TH=[0.17,0.30,0.50,0.64]
    return RAMP[sum(L>t for t in TH)]
def hx(c): return tuple(int(c[i:i+2],16) for i in (1,3,5))
def mix(c1,c2,t):
    a,b=hx(c1),hx(c2); return "#"+"".join(f"{round(p+(q-p)*t):02x}" for p,q in zip(a,b))
BASE={'muzzle':("#07323c","#0d5563"),'mane':("#0b4f5c","#0f766e"),'pale':("#9ef0e2","#e3fdf8"),'face':("#0f9e94","#3fdcc8"),'brow':("#1fb9a9","#52dccb"),'mask':("#0c7d76","#139c92")}
def zone_of(x,y):
    for k,P_ in reversed(ZONES):
        if inside(P_,x,y): return k
    return 'face'
F=[]
for t in tri.simplices:
    cx,cy=P[t].mean(axis=0)
    if not inside(C,cx,cy): continue
    z=zone_of(cx,cy)
    lo,hi=BASE[z]
    # luz vinda do alto e da frente (focinho), com variação sutil por faceta
    light=0.55*(1-cy/470)+0.35*(1-min(1,cx/420))+rnd.uniform(-0.12,0.12)
    F.append((t, mix(lo,hi,max(0,min(1,light))), cx, cy))
# espelha e encaixa no quadro 160x100 da logo
S=92/430
def T(x,y): return (150-(x-38)*S, 4+(y-30)*S)
out=[]
for t,col,cx,cy in F:
    q=[T(*P[i]) for i in t]
    # estilhaços: o pescoço (esquerda) se solta e se desfaz
    gx=T(cx,cy)[0]
    k=max(0,(78-gx)/40)
    if k>0 and rnd.random()<min(1,k*1.25):
        mx=sum(p[0] for p in q)/3; my=sum(p[1] for p in q)/3
        dx=-k*22-rnd.uniform(0,10)*k; dy=-k*6+rnd.uniform(-5,5)*k
        sc=max(0.25,1-k*0.8)
        q=[(mx+dx+(p[0]-mx)*sc, my+dy+(p[1]-my)*sc) for p in q]
        op=max(0.12,1-k*0.95)
    else: op=1
    out.append(f'<polygon points="{" ".join(f"{x:.2f},{y:.2f}" for x,y in q)}" fill="{col}" stroke="{col}" stroke-width="0.25" stroke-linejoin="round"{"" if op==1 else f" fill-opacity=\"{op:.2f}\" stroke-opacity=\"{op:.2f}\""}/>')
eyes="".join(f'<circle cx="{T(x,y)[0]:.2f}" cy="{T(x,y)[1]:.2f}" r="1.25" fill="#04232b"/>' for x,y in EYES)
open(sys.argv[2] if len(sys.argv) > 2 else 'guara-logo.svg','w').write(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 100" role="img" aria-label="Guará">{"".join(out)}{eyes}</svg>\n')
