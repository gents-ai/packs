//! A minimal PDF writer for tests and the fixture generator: uncompressed
//! pages with standard fonts and raw images, written with a valid xref table.

pub struct Img {
    pub w: u32,
    pub h: u32,
    pub gray: bool,
    pub data: Vec<u8>,
    /// Store the pixels Flate-compressed.
    pub flate: bool,
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).expect("writing to a Vec cannot fail");
    enc.finish().expect("finishing a Vec writer cannot fail")
}

pub struct PageSpec<'a> {
    pub w: f32,
    pub h: f32,
    pub content: String,
    pub images: Vec<(&'a str, &'a Img)>,
}

/// One text run: `BT /<font> <size> Tf <x> <y> Td (<text>) Tj ET`.
/// Fonts: F1 Helvetica, F2 Helvetica-Bold, F3 Times-Roman.
pub fn text(font: &str, size: f32, x: f32, y: f32, s: &str) -> String {
    let esc = s
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)");
    format!("BT /{font} {size} Tf {x} {y} Td ({esc}) Tj ET\n")
}

/// Draws image resource `name` into the box at (x, y) of size w by h points.
pub fn draw(name: &str, x: f32, y: f32, w: f32, h: f32) -> String {
    format!("q {w} 0 0 {h} {x} {y} cm /{name} Do Q\n")
}

pub fn pdf(pages: &[PageSpec<'_>]) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![Vec::new(); 5];
    objs[2] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec();
    objs[3] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_vec();
    objs[4] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".to_vec();
    let mut kids = Vec::new();
    for page in pages {
        let mut xobjects = String::new();
        for (name, img) in &page.images {
            let id = objs.len() + 1;
            let cs = if img.gray {
                "/DeviceGray"
            } else {
                "/DeviceRGB"
            };
            let data = if img.flate {
                deflate(&img.data)
            } else {
                img.data.clone()
            };
            let filter = if img.flate {
                "/Filter /FlateDecode "
            } else {
                ""
            };
            let mut obj = format!("<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {cs} /BitsPerComponent 8 {filter}/Length {} >>\nstream\n", img.w, img.h, data.len()).into_bytes();
            obj.extend_from_slice(&data);
            obj.extend_from_slice(b"\nendstream");
            objs.push(obj);
            xobjects.push_str(&format!("/{name} {id} 0 R "));
        }
        let content_id = objs.len() + 1;
        let mut content = format!("<< /Length {} >>\nstream\n", page.content.len()).into_bytes();
        content.extend_from_slice(page.content.as_bytes());
        content.extend_from_slice(b"\nendstream");
        objs.push(content);
        let page_id = objs.len() + 1;
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents {content_id} 0 R /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> /XObject << {xobjects}>> >> >>", page.w, page.h).into_bytes());
        kids.push(format!("{page_id} 0 R"));
    }
    objs[0] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
    objs[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        kids.len()
    )
    .into_bytes();
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A page holding one full-page gray image: what a scanner produces.
pub fn scan(w: u32, h: u32, gray: Vec<u8>, page_w: f32, page_h: f32) -> Vec<u8> {
    let img = Img {
        w,
        h,
        gray: true,
        data: gray,
        flate: true,
    };
    pdf(&[PageSpec {
        w: page_w,
        h: page_h,
        content: draw("Im1", 0.0, 0.0, page_w, page_h),
        images: vec![("Im1", &img)],
    }])
}
