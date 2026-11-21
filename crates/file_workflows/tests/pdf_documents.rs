//! Real document/PDF engine tests. Explicitly opt in after installing workflow
//! dependencies; a selected test fails if a required tool or module is missing.
use std::path::PathBuf;
use std::process::Command;

fn python() -> PathBuf {
    if let Some(path) = std::env::var_os("VIDEO_TOOLKIT_PYTHON") {
        return path.into();
    }
    let local = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.toolkit-venv/bin/python");
    if local.is_file() {
        return local;
    }
    PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
}

fn check(code: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(python())
        .arg("-c")
        .arg(code)
        .env("PDF_WORKER", root.join("python/pdf_worker.py"))
        .env("DOC_WORKER", root.join("python/document_worker.py"))
        .output()
        .expect("Python is required for PDF/document tests");
    assert!(
        output.status.success(),
        "PDF/document check failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Python PDF/image modules, ImageMagick HEIC and Ghostscript; run explicitly with --ignored"]
fn pdf_operations_preserve_page_selection_structure_and_validate_real_outputs() {
    check(
        r#"
import importlib.util, json, os, pathlib, subprocess, sys, tempfile
from pypdf import PdfReader
from reportlab.pdfgen import canvas
from PIL import Image
worker=pathlib.Path(os.environ['PDF_WORKER'])
spec=importlib.util.spec_from_file_location('pdf_worker',worker); mod=importlib.util.module_from_spec(spec);spec.loader.exec_module(mod)
assert mod.pages('3,1-2',3)==[2,0,1]
for invalid in ['0','2-1','4','1,','x']:
 try: mod.pages(invalid,3)
 except ValueError: pass
 else: raise AssertionError(invalid)
with tempfile.TemporaryDirectory() as directory:
 root=pathlib.Path(directory).resolve(); source=root/'source.pdf'; other=root/'second.pdf'
 for path,count in [(source,3),(other,1)]:
  doc=canvas.Canvas(str(path),pagesize=(300,200))
  for i in range(count): doc.drawString(20,100,f'Fixture page {i+1}'); doc.showPage()
  doc.save()
 original=source.read_bytes(); seq=0
 def run(kind,options=None,inputs=None):
  global seq
  seq+=1; stage=root/f'out-{seq}';stage.mkdir()
  request={'kind':kind,'inputs':[str(x) for x in (inputs or [source])],'staging':str(stage),'options':options or {}}
  p=subprocess.run([sys.executable,str(worker)],input=json.dumps(request),text=True,capture_output=True,timeout=60)
  assert p.returncode==0,(kind,p.stderr)
  result=json.loads(p.stdout);outputs=[pathlib.Path(x) for x in result['outputs']]
  assert outputs and all(x.is_file() and x.is_relative_to(stage) for x in outputs)
  return outputs
 assert len(PdfReader(run('pdf_merge',inputs=[source,other])[0]).pages)==4
 split=run('pdf_split',{'ranges':['1-2','3']});assert [len(PdfReader(x).pages) for x in split]==[2,1]
 rotated=PdfReader(run('pdf_rotate',{'pages':'2','angle':90})[0]);assert [p.rotation for p in rotated.pages]==[0,90,0]
 marked=PdfReader(run('pdf_watermark',{'text':'REVIEW','pages':'2'})[0]);assert 'REVIEW' not in marked.pages[0].extract_text();assert 'REVIEW' in marked.pages[1].extract_text()
 metadata=PdfReader(run('pdf_metadata',{'strip':True,'title':'Local fixture','author':'Toolkit'})[0]);assert metadata.metadata.title=='Local fixture';assert metadata.metadata.author=='Toolkit'
 compressed=run('pdf_compress')[0];assert len(PdfReader(compressed).pages)==3;assert compressed.stat().st_size<=source.stat().st_size
 text=run('pdf_to_text',{'pages':'1,3'})[0].read_text();assert 'Fixture page 1' in text and 'Fixture page 3' in text and 'Fixture page 2' not in text
 for fmt in ['png','jpg']:
  images=run('pdf_to_images',{'format':fmt,'pages':'2','dpi':72});assert len(images)==1
  with Image.open(images[0]) as image: image.load();assert image.size==(300,200)
 pictures=[]
 for i in range(2):
  path=root/f'image-{i}.png';Image.new('RGBA',(40,30),(255,0,0,100)).save(path);pictures.append(path)
 assert len(PdfReader(run('images_to_pdf',inputs=pictures)[0]).pages)==2
 heic=root/'phone.heic';subprocess.run(['magick',str(pictures[0]),str(heic)],check=True,capture_output=True)
 assert len(PdfReader(run('images_to_pdf',inputs=[heic])[0]).pages)==1
 assert source.read_bytes()==original
 print('PDF operations verified with real fixtures')
"#,
    );
}

#[test]
#[ignore = "requires Pandoc, LibreOffice and Python document modules; run explicitly with --ignored"]
fn documents_convert_real_semantic_content_and_reject_remote_resources() {
    check(
        r#"
import json, os, pathlib, subprocess, sys, tempfile, zipfile
from pypdf import PdfReader
from docx import Document
from PIL import Image
worker=pathlib.Path(os.environ['DOC_WORKER'])
with tempfile.TemporaryDirectory() as directory:
 root=pathlib.Path(directory).resolve();source=root/'outline.md';source.write_text('# Local fixture\n\nA useful paragraph.\n\n| Item | Count |\n| --- | --- |\n| Alpha | 3 |\n',encoding='utf-8')
 original=source.read_bytes();seq=0
 def run(src,fmt):
  global seq
  seq+=1;stage=root/f'out-{seq}';stage.mkdir()
  request={'kind':'document_convert','inputs':[str(src)],'staging':str(stage),'options':{'format':fmt}}
  p=subprocess.run([sys.executable,str(worker)],input=json.dumps(request),text=True,capture_output=True,timeout=220)
  assert p.returncode==0,(src.suffix,fmt,p.stderr)
  output=pathlib.Path(json.loads(p.stdout)['outputs'][0]);assert output.stat().st_size>0
  return output
 outputs={fmt:run(source,fmt) for fmt in ['txt','md','html','docx','pdf','epub']}
 assert 'useful paragraph' in outputs['txt'].read_text()
 doc=Document(outputs['docx']);assert any('Local fixture' in p.text for p in doc.paragraphs);assert doc.tables[0].cell(1,0).text=='Alpha'
 assert 'useful paragraph' in ''.join(p.extract_text() or '' for p in PdfReader(outputs['pdf']).pages)
 with zipfile.ZipFile(outputs['epub']) as z:assert z.read('mimetype')==b'application/epub+zip'
 for src in [outputs['docx'],outputs['epub'],outputs['html'],outputs['txt'],outputs['pdf']]:assert 'useful paragraph' in run(src,'txt').read_text()
 import pypandoc
 odt=root/'source.odt';subprocess.run([pypandoc.get_pandoc_path(),'--sandbox','--from=gfm','--to=odt',str(source),'--output='+str(odt)],check=True,capture_output=True);assert 'useful paragraph' in run(odt,'txt').read_text()
 rtf=root/'source.rtf';rtf.write_text(r'{\rtf1\ansi A useful paragraph.}');assert 'useful paragraph' in run(rtf,'txt').read_text()
 # Every supported input family is exercised through each of the six targets.
 for src in [source,outputs['docx'],outputs['epub'],outputs['html'],outputs['txt'],outputs['pdf'],rtf,odt]:
  for fmt in ['txt','md','html','docx','pdf','epub']:
   converted=run(src,fmt)
   if fmt=='pdf': assert 'useful paragraph' in ''.join(page.extract_text() or '' for page in PdfReader(converted).pages)
 # Embedded DOCX pictures must survive the semantic HTML intermediary.
 image=root/'image.png';Image.new('RGB',(12,8),'red').save(image)
 picture_doc=Document();picture_doc.add_paragraph('An embedded picture');picture_doc.add_picture(str(image));image_doc=root/'image.docx';picture_doc.save(image_doc)
 embedded=run(image_doc,'docx')
 with zipfile.ZipFile(embedded) as z:assert any(n.startswith('word/media/') for n in z.namelist())
 hostile=root/'remote.html';hostile.write_text('<html><body><iframe src="file:///etc/passwd"></iframe><script>bad()</script><p>safe text</p><img src="http://127.0.0.1:1/never-fetch"></body></html>')
 seq+=1;stage=root/f'out-{seq}';stage.mkdir()
 p=subprocess.run([sys.executable,str(worker)],input=json.dumps({'kind':'document_convert','inputs':[str(hostile)],'staging':str(stage),'options':{'format':'html'}}),text=True,capture_output=True,timeout=30)
 assert p.returncode!=0 and not p.stdout.strip()
 assert 'resource loading is disabled' in p.stderr
 assert source.read_bytes()==original
 print('Six document targets and structured inputs verified')
"#,
    );
}

#[test]
#[ignore = "requires installed Python PDF/document modules and Pandoc; run explicitly with --ignored"]
fn public_execute_publishes_verified_pdf_and_docx_without_overwrite_or_staging_residue() {
    use file_workflows::{execute, run_queue, CancelToken, WorkflowKind, WorkflowRequest};
    use serde_json::json;
    use std::fs;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let fixture = Command::new(python())
        .arg("-c")
        .arg(r#"from pathlib import Path; import sys; from reportlab.pdfgen import canvas; p=Path(sys.argv[1]); c=canvas.Canvas(str(p/'one.pdf')); c.drawString(30,700,'Public API fixture'); c.save(); c=canvas.Canvas(str(p/'second.pdf')); c.drawString(30,700,'Second batch fixture'); c.save(); (p/'notes.md').write_text('# Public API\n\nVerified document content.\n',encoding='utf-8')"#)
        .arg(&root).output().unwrap();
    assert!(
        fixture.status.success(),
        "{}",
        String::from_utf8_lossy(&fixture.stderr)
    );
    let source = root.join("one.pdf");
    let original = fs::read(&source).unwrap();
    let pdf_request = WorkflowRequest {
        kind: WorkflowKind::PdfMerge,
        inputs: vec![source.clone(), source.clone()],
        output_dir: root.join("pdf-outputs"),
        options: json!({}),
    };
    let mut progress = Vec::new();
    let pdf = execute(&pdf_request, &CancelToken::default(), |event| {
        progress.push(event)
    })
    .unwrap();
    assert_eq!(pdf.outputs.len(), 1);
    assert_eq!(
        pdf.outputs[0].parent(),
        Some(pdf_request.output_dir.as_path())
    );
    assert_eq!(progress.last().unwrap().fraction, 1.0);
    let output_bytes = fs::read(&pdf.outputs[0]).unwrap();
    assert!(execute(&pdf_request, &CancelToken::default(), |_| {})
        .unwrap_err()
        .to_string()
        .contains("already exists"));
    assert_eq!(fs::read(&pdf.outputs[0]).unwrap(), output_bytes);
    // Desktop batches submit separate jobs to one output directory. Each PDF
    // operation must retain the source name to avoid collisions between jobs.
    let batch_dir = root.join("batch-outputs");
    let mut batch = Vec::new();
    for kind in [WorkflowKind::PdfRotate, WorkflowKind::PdfToImages] {
        for input in [source.clone(), root.join("second.pdf")] {
            batch.push(WorkflowRequest {
                kind,
                inputs: vec![input],
                output_dir: batch_dir.clone(),
                options: if kind == WorkflowKind::PdfRotate {
                    json!({"angle":90})
                } else {
                    json!({"dpi":72})
                },
            });
        }
    }
    let report = run_queue(&batch, &CancelToken::default(), |_| {});
    assert!(report.success(), "{report:?}");
    let outputs: Vec<_> = report.jobs.iter().flat_map(|job| &job.outputs).collect();
    assert_eq!(outputs.len(), 4);
    for name in [
        "one-rotate.pdf",
        "second-rotate.pdf",
        "one-page-0001.png",
        "second-page-0001.png",
    ] {
        assert!(
            batch_dir.join(name).is_file(),
            "Missing batch output {name}"
        );
    }
    let validation = Command::new(python()).arg("-c")
        .arg("import sys; from pathlib import Path; from pypdf import PdfReader; from PIL import Image; p=Path(sys.argv[1]); assert PdfReader(p/'one-rotate.pdf').pages[0].rotation==90; assert PdfReader(p/'second-rotate.pdf').pages[0].rotation==90; Image.open(p/'one-page-0001.png').load(); Image.open(p/'second-page-0001.png').load()")
        .arg(&batch_dir).output().unwrap();
    assert!(
        validation.status.success(),
        "{}",
        String::from_utf8_lossy(&validation.stderr)
    );
    let doc_request = WorkflowRequest {
        kind: WorkflowKind::DocumentConvert,
        inputs: vec![root.join("notes.md")],
        output_dir: root.join("doc-outputs"),
        options: json!({"format":"docx"}),
    };
    let doc = execute(&doc_request, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(doc.outputs.len(), 1);
    let validation = Command::new(python()).arg("-c")
        .arg("import sys; from pypdf import PdfReader; from docx import Document; assert len(PdfReader(sys.argv[1]).pages)==2; assert any('Verified document content' in p.text for p in Document(sys.argv[2]).paragraphs)")
        .arg(&pdf.outputs[0]).arg(&doc.outputs[0]).output().unwrap();
    assert!(
        validation.status.success(),
        "{}",
        String::from_utf8_lossy(&validation.stderr)
    );
    for output_dir in [&pdf_request.output_dir, &doc_request.output_dir, &batch_dir] {
        assert!(fs::read_dir(output_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".video-toolkit-")));
    }
    assert_eq!(fs::read(&source).unwrap(), original);
    let cancelled = CancelToken::default();
    cancelled.cancel();
    assert!(execute(&doc_request, &cancelled, |_| {}).is_err());
}
