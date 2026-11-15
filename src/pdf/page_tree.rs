//! PDF page-tree inheritance (`MediaBox`, `CropBox`, `Rotate`) and
//! inherited `Resources` / `Font` dictionary mutation.

use anyhow::Result;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::HashSet;

pub(super) fn resolve<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    match obj {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

/// Outcome of visiting one node while walking a `Parent` chain.
enum ParentVisit<T> {
    /// Value found; stop walking and return it.
    Found(T),
    /// No usable value on this node; walk to its parent.
    Continue,
    /// Malformed value on this node; abort instead of inheriting.
    Stop,
}

/// Walk the `Parent` chain from `page_id`, visiting each node's
/// dictionary until `visit` finds a value or aborts. Cycles and
/// non-dictionary nodes end the walk with `None`.
fn parent_chain_find<T>(
    doc: &Document,
    page_id: ObjectId,
    mut visit: impl FnMut(ObjectId, &Dictionary) -> ParentVisit<T>,
) -> Option<T> {
    let mut current = Some(page_id);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return None;
        }
        let dict = doc.get_dictionary(id).ok()?;
        match visit(id, dict) {
            ParentVisit::Found(value) => return Some(value),
            ParentVisit::Continue => {}
            ParentVisit::Stop => return None,
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|obj| obj.as_reference().ok());
    }
    None
}

/// Walk the `Parent` chain looking for an inherited array entry.
/// A present-but-malformed array aborts the walk rather than
/// silently inheriting an ancestor's box.
fn inherited_array(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<[f32; 4]> {
    parent_chain_find(doc, page_id, |_id, dict| {
        let Ok(obj) = dict.get(key) else {
            return ParentVisit::Continue;
        };
        let Some(Object::Array(items)) = resolve(doc, obj) else {
            return ParentVisit::Continue;
        };
        if items.len() < 4 {
            return ParentVisit::Continue;
        }
        let mut out = [0.0_f32; 4];
        for (slot, item) in out.iter_mut().zip(items.iter()) {
            let Some(value) = resolve(doc, item).and_then(|obj| obj.as_float().ok()) else {
                return ParentVisit::Stop;
            };
            *slot = value;
        }
        ParentVisit::Found(out)
    })
}

pub(super) fn inherited_int(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<i64> {
    parent_chain_find(doc, page_id, |_id, dict| {
        dict.get(key)
            .ok()
            .and_then(|obj| resolve(doc, obj))
            .and_then(|value| value.as_i64().ok())
            .map_or(ParentVisit::Continue, ParentVisit::Found)
    })
}

/// Effective visible page box: the `CropBox` intersected with the
/// `MediaBox`, matching what hayro renders. `CropBox` defaults to
/// `MediaBox` when absent (and vice versa). Disjoint boxes clamp to a
/// zero-area box at the max origin, mirroring hayro's intersection.
pub(super) fn page_box(doc: &Document, page_id: ObjectId) -> Option<(f32, f32, f32, f32)> {
    let media = inherited_array(doc, page_id, b"MediaBox");
    let crop = inherited_array(doc, page_id, b"CropBox");
    match (media, crop) {
        (Some([mx0, my0, mx1, my1]), Some([cx0, cy0, cx1, cy1])) => {
            let x0 = mx0.max(cx0);
            let y0 = my0.max(cy0);
            Some((x0, y0, mx1.min(cx1).max(x0), my1.min(cy1).max(y0)))
        }
        (Some([x0, y0, x1, y1]), None) | (None, Some([x0, y0, x1, y1])) => Some((x0, y0, x1, y1)),
        (None, None) => None,
    }
}

#[derive(Clone, Copy)]
pub(super) enum ResourcesTarget {
    /// Resources is a referenced object.
    Reference(ObjectId),
    /// Resources is a direct dictionary on this tree node.
    Direct(ObjectId),
}

pub(super) fn find_resources_target(doc: &Document, page_id: ObjectId) -> Option<ResourcesTarget> {
    parent_chain_find(doc, page_id, |id, dict| match dict.get(b"Resources") {
        Ok(Object::Reference(res_id)) => ParentVisit::Found(ResourcesTarget::Reference(*res_id)),
        Ok(Object::Dictionary(_)) => ParentVisit::Found(ResourcesTarget::Direct(id)),
        _ => ParentVisit::Continue,
    })
}

fn resources_dict_mut(doc: &mut Document, target: ResourcesTarget) -> Result<&mut Dictionary> {
    match target {
        ResourcesTarget::Reference(id) => Ok(doc.get_dictionary_mut(id)?),
        ResourcesTarget::Direct(node_id) => {
            let node = doc.get_dictionary_mut(node_id)?;
            let resources = node.get_mut(b"Resources")?;
            Ok(resources.as_dict_mut()?)
        }
    }
}

/// Insert `alias -> font_id` into the page's effective resources
/// without shadowing inherited entries: an inherited resources
/// dictionary is mutated in place, and a page-local one is created
/// only when no ancestor defines resources at all.
pub(super) fn insert_font_entry(
    doc: &mut Document,
    page_id: ObjectId,
    alias: &[u8],
    font_id: ObjectId,
) -> Result<()> {
    let Some(target) = find_resources_target(doc, page_id) else {
        let resources = doc.get_or_create_resources(page_id)?;
        let resources = resources.as_dict_mut()?;
        let mut font = Dictionary::new();
        font.set(alias.to_vec(), Object::Reference(font_id));
        resources.set("Font", font);
        return Ok(());
    };
    // Font sub-dictionary may itself be a reference; resolve first so
    // the borrow of the resources dictionary is not held across it.
    let font_ref = {
        let resources = resources_dict_mut(doc, target)?;
        match resources.get(b"Font") {
            Ok(Object::Reference(id)) => Some(*id),
            _ => None,
        }
    };
    if let Some(font_dict_id) = font_ref {
        doc.get_dictionary_mut(font_dict_id)?
            .set(alias.to_vec(), Object::Reference(font_id));
        return Ok(());
    }
    let resources = resources_dict_mut(doc, target)?;
    if let Ok(font_obj) = resources.get_mut(b"Font") {
        font_obj
            .as_dict_mut()?
            .set(alias.to_vec(), Object::Reference(font_id));
    } else {
        let mut font = Dictionary::new();
        font.set(alias.to_vec(), Object::Reference(font_id));
        resources.set("Font", font);
    }
    Ok(())
}
