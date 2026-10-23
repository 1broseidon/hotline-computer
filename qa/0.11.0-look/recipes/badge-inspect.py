import ctypes as C,json,subprocess,re
X=C.CDLL('libX11.so.6');S=C.CDLL('libXext.so.6')
X.XOpenDisplay.restype=C.c_void_p
X.XOpenDisplay.argtypes=[C.c_char_p]
d=X.XOpenDisplay(None)
assert d
X.XDefaultRootWindow.restype=C.c_ulong
X.XDefaultRootWindow.argtypes=[C.c_void_p]
root=X.XDefaultRootWindow(d)
X.XQueryTree.argtypes=[C.c_void_p,C.c_ulong,C.POINTER(C.c_ulong),C.POINTER(C.c_ulong),C.POINTER(C.POINTER(C.c_ulong)),C.POINTER(C.c_uint)]
X.XFetchName.argtypes=[C.c_void_p,C.c_ulong,C.POINTER(C.c_char_p)]
X.XFree.argtypes=[C.c_void_p]
S.XShapeGetRectangles.argtypes=[C.c_void_p,C.c_ulong,C.c_int,C.POINTER(C.c_int),C.POINTER(C.c_int)]
S.XShapeGetRectangles.restype=C.c_void_p
rr=C.c_ulong();parent=C.c_ulong();children=C.POINTER(C.c_ulong)();count=C.c_uint()
X.XQueryTree(d,root,C.byref(rr),C.byref(parent),C.byref(children),C.byref(count))
found=[]
for i in range(count.value):
 name=C.c_char_p();wid=children[i]
 X.XFetchName(d,wid,C.byref(name))
 if name.value==b'Hotline sealed fill':
  n=C.c_int();order=C.c_int();rects=S.XShapeGetRectangles(d,wid,2,C.byref(n),C.byref(order))
  info=subprocess.check_output(['xwininfo','-id',str(wid)],text=True)
  get=lambda label:int(re.search(label+r':\s+(-?\d+)',info)[1])
  found.append({'input_rectangles':n.value,'override_redirect':'Override Redirect State: yes' in info,'bounds':[get('Absolute upper-left X'),get('Absolute upper-left Y'),get('Width'),get('Height')]})
  if rects:X.XFree(rects)
 if name:X.XFree(name)
if children:X.XFree(children)
print(json.dumps(found))
