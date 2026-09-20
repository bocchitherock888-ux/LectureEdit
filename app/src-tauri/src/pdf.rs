use serde::Deserialize;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::time::Duration;
use tauri::Manager;
use uuid::Uuid;

const MAX_PDF_PAGES: usize = 500;
const MAX_CAPTURE_BYTES: usize = 256 * 1024 * 1024;
const A4_CSS_WIDTH: f64 = 210.0 * 96.0 / 25.4;
const A4_CSS_HEIGHT: f64 = 297.0 * 96.0 / 25.4;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfPageRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn checked_page_rects(
    page_count: usize,
    page_rects: Vec<PdfPageRect>,
) -> Result<Vec<PdfPageRect>, String> {
    if page_count == 0 || page_count > MAX_PDF_PAGES || page_rects.len() != page_count {
        return Err("PDF 页数无效".into());
    }
    for rect in &page_rects {
        let values = [rect.x, rect.y, rect.width, rect.height];
        if values.iter().any(|value| !value.is_finite())
            || rect.x < 0.0
            || rect.y < 0.0
            || (rect.width - A4_CSS_WIDTH).abs() > 3.0
            || (rect.height - A4_CSS_HEIGHT).abs() > 3.0
        {
            return Err("PDF 页面尺寸无效".into());
        }
    }
    Ok(page_rects)
}

fn output_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.file_name().is_none()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
    {
        return Err("请选择以 .pdf 结尾的保存位置".into());
    }
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err("PDF 保存文件夹不存在".into());
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("LectureEdit.pdf");
    path.with_file_name(format!(".{file_name}.part-{}", Uuid::new_v4()))
}

#[cfg(target_os = "macos")]
fn capture_page(app: &tauri::AppHandle, rect: PdfPageRect) -> Result<Vec<u8>, String> {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSData, NSError, NSPoint, NSRect, NSSize};
    use objc2_web_kit::WKPDFConfiguration;

    let webview = app
        .get_webview_window("main")
        .ok_or("找不到用于生成 PDF 的窗口")?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);

    webview
        .with_webview(move |platform| unsafe {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = sender.send(Err("PDF 生成任务未在主线程运行".to_string()));
                return;
            };
            let view: &objc2_web_kit::WKWebView = &*platform.inner().cast();
            let configuration = WKPDFConfiguration::new(mtm);
            configuration.setRect(NSRect::new(
                NSPoint::new(rect.x, rect.y),
                NSSize::new(rect.width, rect.height),
            ));
            configuration.setAllowTransparentBackground(false);

            let completion: RcBlock<dyn Fn(*mut NSData, *mut NSError)> =
                RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
                    let result = if error.is_null() {
                        data.as_ref()
                            .map(NSData::to_vec)
                            .filter(|bytes| !bytes.is_empty())
                            .ok_or_else(|| "WebKit 返回了空白 PDF 数据".to_string())
                    } else {
                        Err("WebKit 未能生成 PDF 页面".to_string())
                    };
                    let _ = sender.send(result);
                });
            view.createPDFWithConfiguration_completionHandler(Some(&configuration), &completion);
        })
        .map_err(|_| "无法启动 PDF 生成任务".to_string())?;

    receiver
        .recv_timeout(Duration::from_secs(60))
        .map_err(|_| "PDF 生成超时，请缩短课程内容后重试".to_string())?
}

#[cfg(target_os = "macos")]
fn assemble_a4_pdf(captures: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    use objc2_core_foundation::{CFData, CFMutableData, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGContext, CGDataConsumer, CGDataProvider, CGPDFBox, CGPDFContextBeginPage,
        CGPDFContextClose, CGPDFContextCreate, CGPDFContextEndPage, CGPDFDocument, CGPDFPage,
    };

    const A4_WIDTH: f64 = 595.275_590_551;
    const A4_HEIGHT: f64 = 841.889_763_78;
    let media_box = CGRect::new(CGPoint::ZERO, CGSize::new(A4_WIDTH, A4_HEIGHT));
    let output = CFMutableData::new(None, 0).ok_or("无法创建 PDF 数据缓冲区")?;
    let consumer = CGDataConsumer::with_cf_data(Some(&output)).ok_or("无法创建 PDF 写入器")?;
    let context = unsafe { CGPDFContextCreate(Some(&consumer), &media_box, None) }
        .ok_or("无法创建 PDF 文档")?;

    for capture in captures {
        let data = CFData::from_bytes(capture);
        let provider =
            CGDataProvider::with_cf_data(Some(&data)).ok_or("WebKit PDF 页面无法读取")?;
        let document =
            CGPDFDocument::with_provider(Some(&provider)).ok_or("WebKit PDF 页面无法解析")?;
        if CGPDFDocument::number_of_pages(Some(&document)) != 1 {
            return Err("WebKit 返回了异常的 PDF 页面".into());
        }
        let page = CGPDFDocument::page(Some(&document), 1).ok_or("WebKit PDF 页面缺失")?;
        let source_box = CGPDFPage::box_rect(Some(&page), CGPDFBox::MediaBox);
        if source_box.size.width <= 0.0 || source_box.size.height <= 0.0 {
            return Err("WebKit 返回了无效的 PDF 页面".into());
        }

        unsafe { CGPDFContextBeginPage(Some(&context), None) };
        CGContext::save_g_state(Some(&context));
        let transform =
            CGPDFPage::drawing_transform(Some(&page), CGPDFBox::MediaBox, media_box, 0, true);
        CGContext::concat_ctm(Some(&context), transform);
        CGContext::draw_pdf_page(Some(&context), Some(&page));
        CGContext::restore_g_state(Some(&context));
        CGPDFContextEndPage(Some(&context));
    }
    CGPDFContextClose(Some(&context));
    Ok(output.to_vec())
}

#[cfg(target_os = "macos")]
fn validate_pdf_content(bytes: &[u8], expected_pages: usize) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSData, NSRect, NSString};

    #[link(name = "PDFKit", kind = "framework")]
    extern "C" {
        #[link_name = "OBJC_CLASS_$_PDFDocument"]
        static PDF_DOCUMENT_CLASS: AnyClass;
    }

    let class = unsafe { &PDF_DOCUMENT_CLASS };
    let data = NSData::with_bytes(bytes);
    let allocated: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    let document: Option<Retained<AnyObject>> =
        unsafe { msg_send![allocated, initWithData: &*data] };
    let document = document.ok_or("生成的 PDF 无法解析")?;
    let page_count: usize = unsafe { msg_send![&*document, pageCount] };
    if page_count != expected_pages {
        return Err("生成的 PDF 页数不完整".into());
    }

    for index in 0..page_count {
        let page: Option<Retained<AnyObject>> =
            unsafe { msg_send![&*document, pageAtIndex: index] };
        let page = page.ok_or("生成的 PDF 页面无法读取")?;
        let bounds: NSRect = unsafe { msg_send![&*page, boundsForBox: 0_isize] };
        if (bounds.size.width - 595.275_590_551).abs() > 0.5
            || (bounds.size.height - 841.889_763_78).abs() > 0.5
        {
            return Err(format!("生成的 PDF 第 {} 页不是 A4 尺寸", index + 1));
        }
        let text: Option<Retained<NSString>> = unsafe { msg_send![&*page, string] };
        let has_text = text.as_deref().is_some_and(|value| {
            value
                .to_string()
                .chars()
                .any(|character| !character.is_whitespace())
        });
        if !has_text {
            return Err(format!(
                "生成的 PDF 第 {} 页没有可读取的文字内容",
                index + 1
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn write_from_webview(
    app: tauri::AppHandle,
    path: PathBuf,
    page_rects: Vec<PdfPageRect>,
) -> Result<(), String> {
    use std::io::Write;

    let temporary = temporary_path(&path);
    let result = (|| {
        let mut captures = Vec::with_capacity(page_rects.len());
        let mut captured_bytes = 0_usize;
        for rect in page_rects.iter().copied() {
            let capture = capture_page(&app, rect)?;
            captured_bytes = captured_bytes
                .checked_add(capture.len())
                .filter(|total| *total <= MAX_CAPTURE_BYTES)
                .ok_or("PDF 内容过大，请缩短课程内容后重试")?;
            captures.push(capture);
        }

        let pdf = assemble_a4_pdf(&captures)?;
        validate_pdf_content(&pdf, page_rects.len())?;
        let mut file = std::fs::File::create(&temporary)
            .map_err(|error| format!("PDF 临时文件创建失败：{error}"))?;
        file.write_all(&pdf)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("PDF 临时文件写入失败：{error}"))?;
        let written =
            std::fs::read(&temporary).map_err(|error| format!("PDF 临时文件读取失败：{error}"))?;
        validate_pdf_content(&written, page_rects.len())?;
        std::fs::rename(&temporary, &path).map_err(|error| format!("PDF 保存失败：{error}"))
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(target_os = "macos"))]
fn write_from_webview(
    _app: tauri::AppHandle,
    _path: PathBuf,
    _page_rects: Vec<PdfPageRect>,
) -> Result<(), String> {
    Err("当前平台暂不支持精排 PDF 导出".into())
}

#[tauri::command]
pub async fn export_pdf(
    path: String,
    page_count: usize,
    page_rects: Vec<PdfPageRect>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let path = output_path(&path)?;
    let page_rects = checked_page_rects(page_count, page_rects)?;
    tauri::async_runtime::spawn_blocking(move || write_from_webview(app, path, page_rects))
        .await
        .map_err(|_| "PDF 生成任务意外中断".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> PdfPageRect {
        PdfPageRect {
            x: 0.0,
            y: 0.0,
            width: A4_CSS_WIDTH,
            height: A4_CSS_HEIGHT,
        }
    }

    #[test]
    fn accepts_pdf_extension_case_insensitively() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            output_path(root.path().join("Course.PDF").to_str().unwrap()).unwrap(),
            root.path().join("Course.PDF")
        );
    }

    #[test]
    fn rejects_other_extensions_and_missing_parents() {
        let root = tempfile::tempdir().unwrap();
        assert!(output_path(root.path().join("Course.html").to_str().unwrap()).is_err());
        assert!(output_path(
            root.path()
                .join("missing")
                .join("Course.pdf")
                .to_str()
                .unwrap()
        )
        .is_err());
    }

    #[test]
    fn validates_bounded_a4_page_rects() {
        assert_eq!(checked_page_rects(1, vec![rect()]).unwrap().len(), 1);
        assert!(checked_page_rects(0, Vec::new()).is_err());
        assert!(checked_page_rects(2, vec![rect()]).is_err());
        let mut invalid = rect();
        invalid.height = f64::INFINITY;
        assert!(checked_page_rects(1, vec![invalid]).is_err());
        let mut wrong_size = rect();
        wrong_size.width = 500.0;
        assert!(checked_page_rects(1, vec![wrong_size]).is_err());
        assert!(checked_page_rects(MAX_PDF_PAGES + 1, vec![rect(); MAX_PDF_PAGES + 1]).is_err());
    }
}
