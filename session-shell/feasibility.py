#!/usr/bin/env python3
"""Bounded, read-only feasibility inventory of pinned Hyprland capture headers."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

FILES = (
    '/usr/include/hyprland/protocols/hyprland-toplevel-export-v1.hpp',
    '/usr/include/hyprland/protocols/hyprland-toplevel-mapping-v1.hpp',
    '/usr/include/hyprland/src/protocols/ToplevelExport.hpp',
    '/usr/include/hyprland/src/protocols/ImageCopyCapture.hpp',
    '/usr/share/wayland-protocols/staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml',
)
PIN = 'efb50993780079460b0cbed1363e2166a2de1d9f'


def investigate():
    version = subprocess.run(['Hyprland','--version'],capture_output=True,text=True,check=True,timeout=5).stdout
    if len(version)>4096 or PIN not in version:
        raise ValueError('feasibility source pin mismatch')
    evidence = {}
    for name in FILES:
        path=Path(name)
        if path.stat().st_size > 1 << 20: raise ValueError('source byte ceiling')
        raw=path.read_bytes()
        evidence[name]={'sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw)}
    exports=Path(FILES[0]).read_text()
    xml=Path(FILES[-1]).read_text()
    if 'sendBuffer(' not in exports or 'wl_buffer' not in xml or 'attach_buffer' not in xml:
        raise ValueError('capture boundary differs; re-review required')
    return {'schemaVersion':1,'backend':'Hyprland 0.56.2','commit':PIN,'version':version.strip(),
            'reviewKind':'installed source/header/protocol investigation; no running desktop IPC or native input exercised',
            'evidence':evidence,'captureBoundary':'buffer formats, dimensions and copy into wl_buffer',
            'association':'toplevel mapping handles need a trusted connection/object/incarnation bridge; titles and IPC addresses alone are insufficient',
            'permissionedInput':'unproven for app-only RUI grants; virtual/IPC input cannot inherit broader session permission',
            'decision':'retain isolated backend only',
            'blockers':['reviewed capture hooks supply buffers rather than strict-profile source vectors',
                        'no implemented trusted mapping from registered application content to all native popup/shell/decoration surfaces',
                        'no owned-session action/cleanup/scale/occlusion equivalence proof for this backend'],
            'possibleNext':'cooperating vector producer plus a separately reviewed compositor integration; never trace or rasterize capture buffers into vectors'}


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
    report=investigate()
    with args.output.open('x') as stream:json.dump(report,stream,indent=2,sort_keys=True);stream.write('\n')


if __name__=='__main__':main()

