"""Chrome reference profiles for hx-native tests (issue #125).
Run: python3 bevel125_chrome.py (needs google-chrome and Pillow).
Renders each fixture page in headless Chrome at 1, 1.25, 1.5 and 2 and writes
tests/fixtures/bevel125_chrome.txt: page box scale edge colours..."""
import math,subprocess
from PIL import Image
import os,tempfile
FIX=os.path.dirname(os.path.abspath(__file__))+'/'
TMP=tempfile.mkdtemp()
PAGES={
 'bevel125':((200,120),{'panel':(8,8,64,40),'sunk':(72,8,128,40),'button':(136,8,192,32),'window':(8,48,128,112)}),
 'inset123':((320,120),{'bevel':(2,2,122,42),'button':(130,2,205,25),'field':(212,2,312,23),'spread':(2,50,32,80),
    'far':(40,50,60,80),'half':(68,50,108,80),'fade':(116,50,156,80),'inside':(2,88,32,112)}),
}
SCALES=(1,1.25,1.5,2)
def whole(v): return abs(v-round(v))<1e-9
lines=['# Headless Chrome pixels (--force-device-scale-factor) of the fixture pages, made by',
       '# bevel125_chrome.py. Columns: page box scale edge, start pixel x y,\n# step dx dy, then colours from',
       '# the outer edge inward: edges at their middle, corners along the diagonal; ceil(4*scale)+1',
       '# pixels. Only boxes on whole device pixels at that scale (Chrome snaps the rest).']
for page,((w,h),boxes) in PAGES.items():
    for s in SCALES:
        png=f'{TMP}/chrome_{page}_s{s}.png'
        subprocess.run(['google-chrome','--headless=new','--no-sandbox','--disable-gpu','--hide-scrollbars','--virtual-time-budget=3000',
            f'--force-device-scale-factor={s}',f'--window-size={w},{h}',f'--screenshot={png}','file://'+FIX+page+'.html'],capture_output=True,timeout=120)
        im=Image.open(png).convert('RGB')
        n=math.ceil(4*s)+1
        for name,box in boxes.items():
            if not all(whole(v*s) for v in box): continue
            x0,y0,x1,y1=[int(round(v*s)) for v in box]
            mx,my=(x0+x1)//2,(y0+y1)//2
            pts={'top':(mx,y0,0,1),'bottom':(mx,y1-1,0,-1),'left':(x0,my,1,0),'right':(x1-1,my,-1,0),
                 'tl':(x0,y0,1,1),'tr':(x1-1,y0,-1,1),'bl':(x0,y1-1,1,-1),'br':(x1-1,y1-1,-1,-1)}
            for e,(x,y,dx,dy) in pts.items():
                lines.append(f'{page} {name} {s} {e} {x} {y} {dx} {dy} '+' '.join('%02x%02x%02x'%im.getpixel((x+i*dx,y+i*dy)) for i in range(n)))
open(FIX+'bevel125_chrome.txt','w').write('\n'.join(lines)+'\n')
print(len(lines))
