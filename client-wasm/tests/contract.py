"""Independent wire fixture and native/Python oracle for the complete public model."""
import json
import subprocess
from adapters.sdk.producer import (blob, scalar, ref, doubles, encode_surface,
    encode_scene, encode_semantic, encode_text, encode_binding, encode_session)
from conformance.full_reference import FullReplica


def fixture(replay):
    wide = (1 << 64) - 1
    semantics = [dict(id=3, version=wide, role=1, children=list(range(4, 10)),
        label='Root 😀אב', states={1:3}, reading_order=wide)]
    values = (False, -(1 << 63), 1.25, '<img src="https://invalid/">')
    for ident, value in enumerate(values, 4):
        semantics.append(dict(id=ident, version=wide, role=4, children=[], value=value,
            label='Value', description='Description', actions=['SetValue'],
            relations=[dict(kind=2, targets=[8])], semantic_group=3, reading_order=ident,
            states={2:3}, completeness=2))
    semantics += [dict(id=8,version=1,role=6,children=[],label='Name'),
        dict(id=9,version=1,role=7,children=[10,11,12],collection_model=dict(
            version=wide,items=[10,11,12],total_count=wide,materialization=2,selected_items=[10])),
        dict(id=10,version=1,role=8,children=[],label='Available',text_model=20,visual_bindings=[30]),
        dict(id=11,version=1,role=8,children=[],text_model=21,completeness=2),
        dict(id=12,version=1,role=8,children=[],text_model=22,sensitivity=3,completeness=3)]
    surface = encode_surface(dict(surface_id=1,revision=wide,size=[40,30],capture_token='owned-contract',
        gaps=[],scene_root=2,semantic_root=3,focused=True,binding_regions=[],stacking_order=-(1<<63)))
    scene = encode_scene(dict(id=2,version=wide,children=[],transform=[1,0,0,1,0,0],primitive=dict(type='Group')))
    texts = [encode_text(dict(id=20+i,version=wide,text_version=wide,status=i+1,
        content='A😀e\u0301אב' if i==0 else None,selection=[0,6] if i==0 else None,
        editable=i==0,selectable=i==0)) for i in range(3)]
    binding = encode_binding(dict(id=30,version=wide,surface_id=1,semantic_id=10,scene_root=2,
        interaction_region=[0,0,40,30]))
    session = encode_session(dict(id=31,version=wide,canvas=[0,0,40,30],focus=10,
        focus_surface=1,focus_generation=wide))
    # A shape cursor has no external resources. Preserve its complete metadata.
    # Parse the outer length with the independent decoder, avoiding assumptions.
    from conformance.semantic_client import decode, binary
    session_body=binary(decode(session,{6}),6)
    cursor=blob(1,doubles([1,2]))+blob(2,doubles([0,0]))+scalar(3,2)
    session=blob(6,session_body+blob(4,cursor))
    records=[surface,scene,*map(encode_semantic,semantics),*texts,binding,session]
    payload=scalar(1,wide)+b''.join(blob(2,r) for r in records)
    request=dict(kind='snapshot',hex=payload.hex())
    output=json.loads(subprocess.check_output([str(replay),'--full-state'],input=json.dumps(request)+'\n',text=True))
    assert output['accepted'], output
    independent=FullReplica(); assert independent.apply('snapshot',payload)
    assert independent.state()==output['state'], 'Python/native full model differs'
    def numbers(value):
        if isinstance(value,bool) or value is None: return value
        if isinstance(value,float) and value.is_integer(): return str(int(value))
        if isinstance(value,(int,float)): return str(value)
        if isinstance(value,list): return list(map(numbers,value))
        if isinstance(value,dict): return {k:numbers(v) for k,v in value.items()}
        return value
    return dict(hex=payload.hex(),state=numbers(output['state']),variants=6)
