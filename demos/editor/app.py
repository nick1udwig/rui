#!/usr/bin/env python3
"""Owned document editor using the reusable cooperating GTK application API."""
import argparse
from pathlib import Path
import sys
sys.path.insert(0,str(Path(__file__).resolve().parents[2]))
import gi
gi.require_version('Gtk','4.0')
from gi.repository import Gtk, Gio
from adapters.sdk.producer import CoherentSource
from adapters.gtk4.application import Controls, RegisteredApplication
from adapters.gtk4.capture import CaptureWidget
from adapters.gtk4.state import NativeRegistry
from adapters.gtk4.native import NativeDispatcher
from adapters.gtk4.styles import STRICT_CSS


class Editor:
    def __init__(self,app,output,*,diagnostics=False):
        self.app=app;self.source=CoherentSource()
        self.documents=[{'title':'Field notes','body':'café Ω 𝄞 é שלום'}, {'title':'Tasks','body':'Read, write, review'}]
        self.selected=0;self.saves=0;self.loading=False
        self.window=Gtk.ApplicationWindow(application=app,title='RUI document editor')
        self.window.set_default_size(620,360)
        provider=Gtk.CssProvider();provider.load_from_data(STRICT_CSS)
        Gtk.StyleContext.add_provider_for_display(self.window.get_display(),provider,Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        self.registry=NativeRegistry(self.source,self.window)
        self.view=RegisteredApplication(app,self.source,self.registry,output,diagnostics=diagnostics)
        source=self.source
        source.register('documents-root',1,lambda:{'label':'Document editor'})
        source.register('document-form',2,lambda:{'label':'Document form'},parent='documents-root')
        layout=Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL,spacing=16)
        for side in ('top','bottom','start','end'):getattr(layout,'set_margin_'+side)(12)
        self.list=Gtk.ListBox();self.list.set_selection_mode(Gtk.SelectionMode.SINGLE)
        scroll=Gtk.ScrolledWindow();scroll.set_min_content_width(170);scroll.set_child(self.list);layout.append(scroll)
        controls=Controls(source,self.registry,'main-content')
        self.rows={}
        controls.collection('documents',self.list,'documents-root',lambda:{'label':'Documents','collection':{
            'items':list(self.rows),'selected_items':[k for k,r in self.rows.items() if r.is_selected()],
            'total_count':len(self.documents),'materialization':1}})
        for index,document in enumerate(self.documents):
            key='document-'+str(index);row=Gtk.ListBoxRow();row.set_child(Gtk.Label(label=document['title']))
            self.list.append(row);self.rows[key]=row
            controls.item(key,row,'documents',lambda d=document:d['title'])
        form=Gtk.Box(orientation=Gtk.Orientation.VERTICAL,spacing=12);layout.append(form)
        self.heading=Gtk.Label(label='Saved 0 times');form.append(self.heading);controls.label('save-status',self.heading,'document-form')
        self.title=Gtk.Entry(text=self.documents[0]['title']);form.append(self.title)
        controls.entry('document-title',self.title,'document-form','Title')
        self.body=Gtk.Entry(text=self.documents[0]['body']);form.append(self.body)
        controls.entry('document-body',self.body,'document-form','Body')
        save=Gtk.Button(label='Save document');form.append(save);controls.button('save-document',save,'document-form')
        save.connect('clicked',self.save)
        self.confirm=Gtk.Window(application=app,title='Document details',transient_for=self.window,modal=True,decorated=False)
        source.register('details-root',1,lambda:{'label':'Document details'})
        details=Gtk.Box(orientation=Gtk.Orientation.VERTICAL,spacing=8)
        label=Gtk.Label(label='Changes belong to this source document.');details.append(label);controls2=Controls(source,self.registry,'details')
        controls2.label('details-label',label,'details-root')
        close=Gtk.Button(label='Close details');details.append(close);controls2.button('close-details',close,'details-root')
        close.connect('clicked',lambda *_:(self.confirm.set_visible(False),self.title.grab_focus(),source.changed()))
        open_details=Gtk.Button(label='Document details');form.append(open_details);controls.button('open-details',open_details,'document-form')
        open_details.connect('clicked',lambda *_:(self.confirm.present(),source.changed()))
        rebuild=Gtk.Button(label='Replace title control');form.append(rebuild);controls.button('replace-control',rebuild,'document-form')
        def replace_control(*_):
            content=self.title.get_text()
            form.remove(self.title);self.registry.retire('document-title')
            self.title=Gtk.Entry(text=content);form.append(self.title)
            controls.entry('document-title',self.title,'document-form','Title')
            self.title.connect('changed',lambda *_:self.edit('title',self.title.get_text()))
            self.title.grab_focus();source.changed()
        rebuild.connect('clicked',replace_control)
        reviewed=Gtk.ToggleButton(label='Reviewed');form.append(reviewed)
        controls.checkbox('reviewed',reviewed,'document-form')
        private=Gtk.Entry(text='local private secret');private.set_visibility(False);form.append(private)
        controls.entry('private-field',private,'document-form','Private field',private=True)
        main=CaptureWidget(source,layout,'main-content','documents-root',output)
        dialog=CaptureWidget(source,details,'details','details-root',output)
        self.window.set_child(main);self.confirm.set_child(dialog)
        self.view.surface('main-content',main,self.window,parent_matches=lambda:self.window.get_child()==main)
        self.view.surface('details',dialog,self.confirm,origin=(180,80),stacking=1,parent='main-content',modal=True,
                          parent_matches=lambda:self.confirm.get_transient_for()==self.window and self.confirm.get_child()==dialog)
        self.list.connect('row-selected',self.select)
        self.title.connect('changed',lambda *_:self.edit('title',self.title.get_text()))
        self.body.connect('changed',lambda *_:self.edit('body',self.body.get_text()))
        self.list.select_row(self.list.get_row_at_index(0))
        self.window.present();self.title.grab_focus()
        self.dispatcher=NativeDispatcher(self.view.integration('main-content'),whole=self.view,watchdog_seconds=0)
        self.window.connect('close-request',lambda *_:(app.quit(),False)[1])
        app.connect('shutdown',lambda *_:self.dispatcher.close())

    def edit(self,field,value):
        if not self.loading:self.documents[self.selected][field]=value
        self.source.changed()

    def save(self,*_):
        self.saves+=1;self.heading.set_text(f'Saved {self.saves} times');self.source.changed()

    def select(self,_list,row):
        if row is None:return
        self.selected=row.get_index();self.loading=True
        try:
            document=self.documents[self.selected];self.title.set_text(document['title']);self.body.set_text(document['body'])
        finally:self.loading=False
        self.source.changed()


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--diagnostics',action='store_true');args=parser.parse_args()
    app=Gtk.Application(application_id='org.rui.DocumentEditor',flags=Gio.ApplicationFlags.NON_UNIQUE)
    app.connect('activate',lambda app:setattr(app,'editor',Editor(app,args.output,diagnostics=args.diagnostics)))
    return app.run(None)


if __name__=='__main__':raise SystemExit(main())

