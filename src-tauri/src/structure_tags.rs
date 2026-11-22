use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ITEMS: usize = 100_000;

#[derive(Debug)]
pub struct RepairResult {
    pub changed: bool,
    pub status: String,
}

// Work on dictionary copies until the complete page-content relationship has
// been checked. Unknown forms, object references and cycles leave tags intact.
struct Plan<'a> {
    doc: &'a Document,
    pages: BTreeSet<ObjectId>,
    visited: BTreeSet<ObjectId>,
    edits: BTreeMap<ObjectId, Dictionary>,
    content: BTreeMap<ObjectId, BTreeSet<i64>>,
    parents: BTreeMap<(ObjectId, i64), ObjectId>,
    orphaned: usize,
    items: usize,
}

fn dictionary<'a>(doc: &'a Document, object: &'a Object) -> Result<&'a Dictionary, String> {
    doc.dereference(object)
        .ok()
        .and_then(|(_, o)| o.as_dict().ok())
        .ok_or_else(|| "结构标签包含无法解析的字典".into())
}

fn inherited_resources(doc: &Document, mut page: ObjectId) -> Option<&Dictionary> {
    let mut seen = BTreeSet::new();
    while seen.insert(page) {
        let node = doc.get_dictionary(page).ok()?;
        if let Ok(resources) = node.get(b"Resources") {
            return dictionary(doc, resources).ok();
        }
        page = node.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

impl Plan<'_> {
    fn count(&mut self, depth: usize) -> Result<(), String> {
        self.items += 1;
        if depth > 128 || self.items > MAX_ITEMS {
            return Err("结构标签过深或过多，已保留原标签".into());
        }
        Ok(())
    }

    fn page_content(&mut self, page: ObjectId) -> Result<&BTreeSet<i64>, String> {
        if !self.content.contains_key(&page) {
            let bytes = self
                .doc
                .get_page_content(page)
                .map_err(|_| "无法读取标签对应的页面内容")?;
            let content = Content::decode(&bytes).map_err(|_| "无法确认页面中的内容编号")?;
            let mut ids = BTreeSet::new();
            for op in content.operations.iter().filter(|op| op.operator == "BDC") {
                if op.operands.len() != 2 {
                    return Err("页面的标记内容参数异常".into());
                }
                let properties = match &op.operands[1] {
                    Object::Dictionary(d) => d,
                    Object::Name(name) => {
                        let resources =
                            inherited_resources(self.doc, page).ok_or("页面缺少标签资源")?;
                        let properties = dictionary(
                            self.doc,
                            resources
                                .get(b"Properties")
                                .map_err(|_| "标签属性资源缺失")?,
                        )?;
                        dictionary(
                            self.doc,
                            properties.get(name).map_err(|_| "找不到标签属性资源")?,
                        )?
                    }
                    _ => return Err("页面的标记内容属性类型异常".into()),
                };
                if let Ok(value) = properties.get(b"MCID") {
                    let id = value.as_i64().map_err(|_| "内容编号不是整数")?;
                    if id < 0 || id as u64 >= MAX_ITEMS as u64 || !ids.insert(id) {
                        return Err("内容编号重复、为负数或超出修复范围".into());
                    }
                }
            }
            self.content.insert(page, ids);
        }
        Ok(&self.content[&page])
    }

    fn content_item(
        &mut self,
        page: Option<ObjectId>,
        mcid: i64,
        parent: ObjectId,
    ) -> Result<bool, String> {
        let page = page.ok_or("内容标签缺少可确认的页面引用")?;
        if mcid < 0 || mcid as u64 >= MAX_ITEMS as u64 {
            return Err("结构标签的内容编号超出修复范围".into());
        }
        if !self.page_content(page)?.contains(&mcid) {
            self.orphaned += 1;
            return Ok(false);
        }
        if self.parents.insert((page, mcid), parent).is_some() {
            return Err("同一内容编号对应多个标签，无法可靠重建索引".into());
        }
        Ok(true)
    }

    fn element(
        &mut self,
        id: ObjectId,
        parent: ObjectId,
        inherited_page: Option<ObjectId>,
        depth: usize,
    ) -> Result<(), String> {
        self.count(depth)?;
        if !self.visited.insert(id) {
            return Err("结构标签包含循环或重复引用".into());
        }
        let mut node = self
            .doc
            .get_dictionary(id)
            .map_err(|_| "结构元素不是字典")?
            .clone();
        if node.get(b"S").and_then(Object::as_name).is_err()
            || node
                .get(b"Type")
                .is_ok_and(|o| o.as_name().ok() != Some(b"StructElem"))
        {
            return Err("结构元素类型无法确认，已保留原标签".into());
        }
        let page = match node.get(b"Pg") {
            Ok(o) => Some(o.as_reference().map_err(|_| "结构标签的页面引用类型异常")?),
            Err(_) => inherited_page,
        };
        if page.is_some_and(|p| !self.pages.contains(&p)) {
            return Err("结构标签指向不存在的页面".into());
        }
        node.set("P", parent);
        if let Ok(children) = node.get(b"K").cloned() {
            if let Some(children) = self.children(&children, id, page, depth + 1)? {
                node.set("K", children);
            } else {
                node.remove(b"K");
            }
        }
        if &node != self.doc.get_dictionary(id).unwrap() {
            self.edits.insert(id, node);
        }
        Ok(())
    }

    fn children(
        &mut self,
        object: &Object,
        parent: ObjectId,
        page: Option<ObjectId>,
        depth: usize,
    ) -> Result<Option<Object>, String> {
        self.count(depth)?;
        let (_, resolved) = self
            .doc
            .dereference(object)
            .map_err(|_| "结构标签子项无法解析")?;
        match resolved {
            Object::Array(children) => {
                let mut kept = Vec::new();
                for child in children {
                    if let Some(child) = self.children(child, parent, page, depth + 1)? {
                        kept.push(child);
                    }
                }
                if kept == *children {
                    return Ok(Some(object.clone()));
                }
                Ok((!kept.is_empty()).then_some(Object::Array(kept)))
            }
            Object::Integer(mcid) => Ok(self
                .content_item(page, *mcid, parent)?
                .then(|| object.clone())),
            Object::Dictionary(node) if node.get(b"S").is_ok() => {
                let id = object.as_reference().map_err(|_| "暂不修复内嵌结构元素")?;
                self.element(id, parent, page, depth + 1)?;
                Ok(Some(object.clone()))
            }
            Object::Dictionary(node)
                if node.get(b"Type").and_then(Object::as_name).ok() == Some(b"MCR") =>
            {
                if node.get(b"Stm").is_ok() || node.get(b"StmOwn").is_ok() {
                    return Err("暂不修复独立内容流中的标签关联".into());
                }
                let page = match node.get(b"Pg") {
                    Ok(o) => Some(o.as_reference().map_err(|_| "内容引用的页面类型异常")?),
                    Err(_) => page,
                };
                if page.is_some_and(|p| !self.pages.contains(&p)) {
                    return Err("内容引用指向不存在的页面".into());
                }
                let mcid = node
                    .get(b"MCID")
                    .and_then(Object::as_i64)
                    .map_err(|_| "内容引用缺少整数编号")?;
                Ok(self
                    .content_item(page, mcid, parent)?
                    .then(|| object.clone()))
            }
            Object::Null => Ok(None),
            _ => Err("暂不修复对象标签或未知类型的标签关联".into()),
        }
    }
}

type ParentIndex = BTreeMap<i64, Vec<Option<ObjectId>>>;

fn read_parent_index(
    doc: &Document,
    object: &Object,
    seen: &mut BTreeSet<ObjectId>,
    depth: usize,
) -> Result<ParentIndex, ()> {
    if depth > 64 || seen.len() > MAX_ITEMS {
        return Err(());
    }
    if let Ok(id) = object.as_reference() {
        if !seen.insert(id) {
            return Err(());
        }
    }
    let node = dictionary(doc, object).map_err(|_| ())?;
    let mut result = BTreeMap::new();
    if let Ok(children) = node.get(b"Kids") {
        if node.get(b"Nums").is_ok() {
            return Err(());
        }
        for child in doc
            .dereference(children)
            .map_err(|_| ())?
            .1
            .as_array()
            .map_err(|_| ())?
        {
            let sub = read_parent_index(doc, child, seen, depth + 1)?;
            if result
                .last_key_value()
                .zip(sub.first_key_value())
                .is_some_and(|((a, _), (b, _))| a >= b)
            {
                return Err(());
            }
            result.extend(sub);
        }
    } else {
        let nums = doc
            .dereference(node.get(b"Nums").map_err(|_| ())?)
            .map_err(|_| ())?
            .1
            .as_array()
            .map_err(|_| ())?;
        if nums.len() % 2 != 0 || nums.len() > MAX_ITEMS * 2 {
            return Err(());
        }
        for pair in nums.chunks_exact(2) {
            let key = pair[0].as_i64().map_err(|_| ())?;
            if key < 0
                || result
                    .last_key_value()
                    .is_some_and(|(&last, _)| last >= key)
            {
                return Err(());
            }
            let array = doc
                .dereference(&pair[1])
                .map_err(|_| ())?
                .1
                .as_array()
                .map_err(|_| ())?;
            if array.len() > MAX_ITEMS {
                return Err(());
            }
            let values = array
                .iter()
                .map(|o| match o {
                    Object::Null => Ok(None),
                    Object::Reference(id) => Ok(Some(*id)),
                    _ => Err(()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            result.insert(key, values);
        }
    }
    if let Ok(limits) = node.get(b"Limits") {
        let limits = doc
            .dereference(limits)
            .map_err(|_| ())?
            .1
            .as_array()
            .map_err(|_| ())?;
        if limits.len() != 2
            || limits[0].as_i64().ok() != result.first_key_value().map(|(&k, _)| k)
            || limits[1].as_i64().ok() != result.last_key_value().map(|(&k, _)| k)
        {
            return Err(());
        }
    } else if depth > 0 {
        return Err(());
    }
    Ok(result)
}

pub fn repair(
    doc: &mut Document,
    confirm_cleanup: impl FnOnce() -> bool,
) -> Result<RepairResult, String> {
    let catalog = doc.catalog().map_err(|_| "文档目录无法读取")?;
    let Ok(root_ref) = catalog.get(b"StructTreeRoot") else {
        return Ok(RepairResult {
            changed: false,
            status: "无结构标签".into(),
        });
    };
    let root_id = root_ref
        .as_reference()
        .map_err(|_| "结构树根不是间接引用，已保留原标签")?;
    let mut root = dictionary(doc, root_ref)?.clone();
    if root
        .get(b"Type")
        .is_ok_and(|o| o.as_name().ok() != Some(b"StructTreeRoot"))
    {
        return Err("结构树根类型异常，已保留原标签".into());
    }
    let pages: BTreeSet<_> = doc.get_pages().into_values().collect();
    if pages.is_empty() {
        return Err("文档没有可确认的页面".into());
    }
    // Page content is the only content scope currently repaired. Do not remove
    // tags belonging to an annotation or a Form that this traversal cannot prove.
    for (&id, object) in &doc.objects {
        let dict = object
            .as_dict()
            .ok()
            .or_else(|| object.as_stream().ok().map(|s| &s.dict));
        if dict.is_some_and(|d| {
            d.get(b"StructParent").is_ok()
                || (!pages.contains(&id) && d.get(b"StructParents").is_ok())
        }) {
            return Err("存在注释或独立内容流的标签关联，已保留原标签".into());
        }
    }
    let mut plan = Plan {
        doc,
        pages,
        visited: BTreeSet::new(),
        edits: BTreeMap::new(),
        content: BTreeMap::new(),
        parents: BTreeMap::new(),
        orphaned: 0,
        items: 0,
    };
    if let Ok(children) = root.get(b"K") {
        let (_, children) = doc
            .dereference(children)
            .map_err(|_| "无法解析结构树子项")?;
        let children = match children {
            Object::Array(a) => a.clone(),
            Object::Dictionary(_) => vec![root.get(b"K").unwrap().clone()],
            Object::Null => vec![],
            _ => return Err("结构树根子项类型异常".into()),
        };
        for child in children {
            plan.element(
                child
                    .as_reference()
                    .map_err(|_| "结构树包含内嵌或无效的根子项")?,
                root_id,
                None,
                0,
            )?;
        }
    }
    if plan.parents.is_empty() && plan.orphaned > 0 {
        // Clear only proven dead tagging. A live MCID anywhere makes wholesale
        // cleanup unsafe, even when the broken structure never references it.
        for page in plan.pages.clone() {
            if !plan.page_content(page)?.is_empty() {
                return Err("页面仍有无法关联的标记内容，已保留原标签".into());
            }
        }
        let count = plan.visited.len();
        let orphaned = plan.orphaned;
        let mut edits = BTreeMap::new();
        for &page in &plan.pages {
            let mut node = doc
                .get_dictionary(page)
                .map_err(|_| "页面字典无法读取")?
                .clone();
            node.remove(b"StructParents");
            if node.get(b"Tabs").and_then(Object::as_name).ok() == Some(b"S") {
                node.remove(b"Tabs");
            }
            edits.insert(page, node);
        }
        let marked = catalog
            .get(b"MarkInfo")
            .ok()
            .map(|o| {
                let mut value = dictionary(doc, o)?.clone();
                value.set("Marked", false);
                Ok::<_, String>(value)
            })
            .transpose()?;
        if !confirm_cleanup() {
            return Ok(RepairResult {
                changed: false,
                status: "未清理，已保留原结构标签".into(),
            });
        }
        // All fallible inspection is complete before the first mutation.
        for (id, node) in edits {
            doc.objects.insert(id, Object::Dictionary(node));
        }
        let catalog = doc.catalog_mut().map_err(|_| "文档目录无法写入")?;
        catalog.remove(b"StructTreeRoot");
        if let Some(marked) = marked {
            catalog.set("MarkInfo", marked);
        }
        return Ok(RepairResult {changed: true, status: format!("已清理 {count} 个失效结构标签（{orphaned} 处内容关联失效），保留扫描图与 OCR；无障碍标签已移除")});
    }

    let mut groups: BTreeMap<ObjectId, Vec<Option<ObjectId>>> = BTreeMap::new();
    let mut total_slots = 0usize;
    for (&(page, mcid), &parent) in &plan.parents {
        let values = groups.entry(page).or_default();
        let length = values.len().max(mcid as usize + 1);
        total_slots += length - values.len();
        if total_slots > MAX_ITEMS * 10 {
            return Err("标签索引过于稀疏，已保留原标签".into());
        }
        values.resize(length, None);
        values[mcid as usize] = Some(parent);
    }
    let mut expected = ParentIndex::new();
    let mut keys = BTreeSet::new();
    for (&page, values) in &groups {
        let mut node = doc
            .get_dictionary(page)
            .map_err(|_| "页面字典无法读取")?
            .clone();
        let key = node
            .get(b"StructParents")
            .and_then(Object::as_i64)
            .ok()
            .filter(|k| *k >= 0 && *k < i64::MAX && !keys.contains(k))
            .unwrap_or_else(|| (0..).find(|k| !keys.contains(k)).unwrap());
        keys.insert(key);
        expected.insert(key, values.clone());
        node.set("StructParents", key);
        if &node != doc.get_dictionary(page).unwrap() {
            plan.edits.insert(page, node);
        }
    }
    for &page in &plan.pages {
        if !groups.contains_key(&page)
            && doc
                .get_dictionary(page)
                .is_ok_and(|d| d.get(b"StructParents").is_ok())
        {
            let mut node = doc.get_dictionary(page).unwrap().clone();
            node.remove(b"StructParents");
            plan.edits.insert(page, node);
        }
    }
    let current = root
        .get(b"ParentTree")
        .ok()
        .map(|o| read_parent_index(doc, o, &mut BTreeSet::new(), 0))
        .transpose();
    let rebuild = match current {
        Ok(Some(index)) => index != expected,
        Ok(None) => !expected.is_empty(),
        Err(_) => true,
    };
    let next_key = keys.last().copied().map_or(0, |k| k + 1);
    if root
        .get(b"ParentTreeNextKey")
        .is_ok_and(|o| o.as_i64().ok().is_none_or(|k| k < next_key))
    {
        root.set("ParentTreeNextKey", next_key);
    }
    root.set("Type", "StructTreeRoot");
    let root_changed = &root
        != doc
            .get_dictionary(root_id)
            .map_err(|_| "结构树根字典无法读取")?;
    if !rebuild && !root_changed && plan.edits.is_empty() {
        return Ok(RepairResult {
            changed: false,
            status: "结构标签正常".into(),
        });
    }
    let count = plan.visited.len();
    let orphaned = plan.orphaned;
    let mut edits = plan.edits;
    if rebuild {
        let mut nums = Vec::new();
        for (key, values) in expected {
            nums.push(Object::Integer(key));
            nums.push(Object::Array(
                values
                    .into_iter()
                    .map(|id| id.map_or(Object::Null, Object::Reference))
                    .collect(),
            ));
        }
        let mut tree = Dictionary::new();
        tree.set("Nums", nums);
        let tree = doc.add_object(tree);
        root.set("ParentTree", tree);
        root.set("ParentTreeNextKey", next_key);
    }
    edits.insert(root_id, root);
    for (id, node) in edits {
        doc.objects.insert(id, Object::Dictionary(node));
    }
    Ok(RepairResult {
        changed: true,
        status: format!("已修复结构标签索引，保留 {count} 个标签，移除 {orphaned} 处失效内容关联"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn fixture(content: &[u8]) -> (Document, ObjectId, ObjectId, ObjectId) {
        let mut doc = Document::with_version("1.5");
        let pages = doc.new_object_id();
        let root = doc.new_object_id();
        let contents = doc.add_object(lopdf::Stream::new(Dictionary::new(), content.to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => contents,
            "MediaBox" => vec![0.into(),0.into(),300.into(),400.into()],
            "StructParents" => 0, "Tabs" => "S"
        });
        doc.objects.insert(
            pages,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1
            }),
        );
        let attributes = doc.add_object(dictionary! {"O" => "Layout", "Placement" => "Block"});
        let element = doc.add_object(
            dictionary! {"S" => "P", "P" => root, "Pg" => page, "K" => 0, "A" => attributes},
        );
        let tree = doc.add_object(
            dictionary! {"Nums" => vec![0.into(), Object::Array(vec![attributes.into()])]},
        );
        doc.objects.insert(
            root,
            Object::Dictionary(dictionary! {
                "Type" => "StructTreeRoot", "K" => vec![element.into()], "ParentTree" => tree
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages, "StructTreeRoot" => root,
            "MarkInfo" => dictionary! {"Marked" => true}, "Lang" => Object::string_literal("zh-CN")
        });
        doc.trailer.set("Root", catalog);
        (doc, page, element, attributes)
    }

    #[test]
    fn wrong_parent_index_is_rebuilt_from_live_content_and_repair_is_idempotent() {
        let (mut doc, page, element, attributes) =
            fixture(b"/P << /MCID 0 >> BDC 0 0 20 20 re f EMC");
        let contents = doc.get_page_content(page).unwrap();
        let result = repair(&mut doc, || panic!("index rebuild must not request cleanup")).unwrap();
        assert!(result.changed && result.status.contains("保留 1 个标签"));
        let root =
            dictionary(&doc, doc.catalog().unwrap().get(b"StructTreeRoot").unwrap()).unwrap();
        let tree = read_parent_index(
            &doc,
            root.get(b"ParentTree").unwrap(),
            &mut BTreeSet::new(),
            0,
        )
        .unwrap();
        assert_eq!(tree[&0], vec![Some(element)]);
        assert_ne!(tree[&0], vec![Some(attributes)]);
        assert_eq!(doc.get_page_content(page).unwrap(), contents);
        assert!(doc
            .get_dictionary(page)
            .unwrap()
            .get(b"StructParents")
            .is_ok());
        let objects = doc.objects.clone();
        assert!(!repair(&mut doc, || false).unwrap().changed);
        assert_eq!(doc.objects, objects);
    }

    #[test]
    fn dead_tags_are_cleared_only_when_the_whole_document_has_no_live_content_ids() {
        let (mut doc, page, _, _) = fixture(b"0 0 20 20 re f");
        let content = doc.get_page_content(page).unwrap();
        let before = doc.objects.clone();
        let kept = repair(&mut doc, || false).unwrap();
        assert!(!kept.changed && kept.status.contains("已保留原结构标签"));
        assert_eq!(doc.objects, before);
        let result = repair(&mut doc, || true).unwrap();
        assert!(result.changed && result.status.contains("无障碍标签已移除"));
        assert!(doc.catalog().unwrap().get(b"StructTreeRoot").is_err());
        assert!(
            !dictionary(&doc, doc.catalog().unwrap().get(b"MarkInfo").unwrap())
                .unwrap()
                .get(b"Marked")
                .unwrap()
                .as_bool()
                .unwrap()
        );
        assert!(doc
            .get_dictionary(page)
            .unwrap()
            .get(b"StructParents")
            .is_err());
        assert!(doc.get_dictionary(page).unwrap().get(b"Tabs").is_err());
        assert_eq!(content, doc.get_page_content(page).unwrap());
        assert!(!repair(&mut doc, || false).unwrap().changed);

        let (mut doc, _, _, _) = fixture(b"/P << /MCID 1 >> BDC 0 0 20 20 re f EMC");
        let objects = doc.objects.clone();
        assert!(repair(&mut doc, || panic!("ambiguous tags must be preserved"))
            .unwrap_err()
            .contains("无法关联"));
        assert_eq!(doc.objects, objects);
    }

    #[test]
    fn mixed_live_and_missing_content_preserves_live_tags_and_named_properties() {
        let (mut doc, page, element, _) = fixture(b"/P /Props BDC 0 0 20 20 re f EMC");
        doc.get_dictionary_mut(page).unwrap().set(
            "Resources",
            dictionary! {
                "Properties" => dictionary! {"Props" => dictionary! {"MCID" => 0}}
            },
        );
        doc.get_dictionary_mut(element)
            .unwrap()
            .set("K", vec![Object::Integer(0), Object::Integer(1)]);
        let result = repair(&mut doc, || false).unwrap();
        assert!(result.changed && result.status.contains("移除 1 处失效"));
        assert_eq!(
            doc.get_dictionary(element)
                .unwrap()
                .get(b"K")
                .unwrap()
                .as_array()
                .unwrap(),
            &vec![Object::Integer(0)]
        );
        assert!(!repair(&mut doc, || false).unwrap().changed);
    }

    #[test]
    fn ambiguous_cycles_duplicate_content_and_unknown_scopes_never_partially_mutate() {
        for content in [
            b"/P << /MCID 0 >> BDC EMC /P << /MCID 0 >> BDC EMC".as_slice(),
            b"/P << /MCID -1 >> BDC EMC",
        ] {
            let (mut doc, _, _, _) = fixture(content);
            let before = doc.objects.clone();
            assert!(repair(&mut doc, || false).is_err());
            assert_eq!(doc.objects, before);
        }
        for kind in 0..4 {
            let (mut doc, _, element, _) = fixture(b"/P << /MCID 0 >> BDC EMC");
            match kind {
                0 => doc.get_dictionary_mut(element).unwrap().set("K", element),
                1 => doc.get_dictionary_mut(element).unwrap().set(
                    "K",
                    dictionary! {"Type" => "MCR", "MCID" => 0, "Stm" => (999,0)},
                ),
                2 => doc
                    .get_dictionary_mut(element)
                    .unwrap()
                    .set("K", dictionary! {"Type" => "OBJR", "Obj" => (999,0)}),
                _ => {
                    doc.add_object(dictionary! {"Type" => "Annot", "StructParent" => 4});
                }
            }
            let before = doc.objects.clone();
            assert!(repair(&mut doc, || false).is_err());
            assert_eq!(doc.objects, before);
        }
    }

    #[test]
    fn duplicate_page_keys_and_missing_number_tree_entries_are_recovered() {
        let (mut doc, page, first, _) = fixture(b"/P << /MCID 0 >> BDC EMC");
        let template = doc.get_dictionary(page).unwrap().clone();
        let second_page = doc.add_object(template);
        let root = doc
            .catalog()
            .unwrap()
            .get(b"StructTreeRoot")
            .unwrap()
            .as_reference()
            .unwrap();
        let second =
            doc.add_object(dictionary! {"S" => "P", "P" => root, "Pg" => second_page, "K" => 0});
        doc.get_dictionary_mut(root).unwrap().set(
            "K",
            vec![Object::Reference(first), Object::Reference(second)],
        );
        let pages = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Parent")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_dictionary_mut(pages).unwrap().set(
            "Kids",
            vec![Object::Reference(page), Object::Reference(second_page)],
        );
        doc.get_dictionary_mut(pages).unwrap().set("Count", 2);
        repair(&mut doc, || false).unwrap();
        let key = |id| {
            doc.get_dictionary(id)
                .unwrap()
                .get(b"StructParents")
                .unwrap()
                .as_i64()
                .unwrap()
        };
        assert_ne!(key(page), key(second_page));
        assert!(!repair(&mut doc, || false).unwrap().changed);
    }
}
