#import <Foundation/Foundation.h>
#import <CoreData/CoreData.h>
#import <objc/runtime.h>
#import <objc/message.h>
#import <dlfcn.h>

static id send0(id obj, SEL sel) {
    return ((id (*)(id, SEL))objc_msgSend)(obj, sel);
}

static id send1(id obj, SEL sel, id a) {
    return ((id (*)(id, SEL, id))objc_msgSend)(obj, sel, a);
}

static id send2(id obj, SEL sel, id a, id b) {
    return ((id (*)(id, SEL, id, id))objc_msgSend)(obj, sel, a, b);
}

static BOOL bool0(id obj, SEL sel) {
    return ((BOOL (*)(id, SEL))objc_msgSend)(obj, sel);
}

static void sendClassUll(Class cls, SEL sel, unsigned long long value) {
    ((void (*)(Class, SEL, unsigned long long))objc_msgSend)(cls, sel, value);
}

static NSString *defaultResultPath(void) {
    NSString *dir = [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/apple-cli"];
    [[NSFileManager defaultManager] createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:nil error:nil];
    return [dir stringByAppendingPathComponent:@"notes-private-result.json"];
}

static void writeResult(NSDictionary *result) {
    NSString *path = [[[NSProcessInfo processInfo] environment] objectForKey:@"APPLE_CLI_NOTES_PRIVATE_RESULT"];
    if (path.length == 0) {
        path = defaultResultPath();
    }
    NSMutableDictionary *payload = [result mutableCopy];
    payload[@"result_path"] = path;
    NSError *jsonError = nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:payload options:NSJSONWritingPrettyPrinted error:&jsonError];
    if (!data) {
        NSLog(@"apple-cli notes private result JSON failed: %@", jsonError);
        return;
    }
    NSError *writeError = nil;
    BOOL ok = [data writeToFile:path options:NSDataWritingAtomic error:&writeError];
    if (!ok) {
        NSLog(@"apple-cli notes private result write failed path=%@ error=%@", path, writeError);
    } else {
        NSLog(@"apple-cli notes private wrote result to %@", path);
    }
}

static NSDictionary *readRequest(void) {
    NSString *requestPath = [[[NSProcessInfo processInfo] environment] objectForKey:@"APPLE_CLI_NOTES_PRIVATE_REQUEST"];
    if (requestPath.length == 0) {
        return @{};
    }
    NSData *data = [NSData dataWithContentsOfFile:requestPath];
    if (!data) {
        return @{@"op": @"error", @"error": [NSString stringWithFormat:@"could not read request path %@", requestPath]};
    }
    NSError *error = nil;
    id json = [NSJSONSerialization JSONObjectWithData:data options:0 error:&error];
    if (![json isKindOfClass:[NSDictionary class]]) {
        return @{@"op": @"error", @"error": [NSString stringWithFormat:@"invalid request JSON: %@", error]};
    }
    return (NSDictionary *)json;
}

static id notesManagedObjectContext(void) {
    dlopen("/System/Library/PrivateFrameworks/NotesShared.framework/NotesShared", RTLD_NOW);
    dlopen("/System/Library/PrivateFrameworks/NotesUI.framework/NotesUI", RTLD_NOW);
    dlopen("/System/Library/PrivateFrameworks/NotesEditor.framework/NotesEditor", RTLD_NOW);

    Class noteContextClass = NSClassFromString(@"ICNoteContext");
    if (!noteContextClass) return nil;
    if ([noteContextClass respondsToSelector:NSSelectorFromString(@"startSharedContextWithOptions:")]) {
        sendClassUll(noteContextClass, NSSelectorFromString(@"startSharedContextWithOptions:"), 0);
    }
    id noteContext = ((id (*)(Class, SEL))objc_msgSend)(noteContextClass, NSSelectorFromString(@"sharedContext"));
    if (!noteContext) return nil;
    return send0(noteContext, NSSelectorFromString(@"managedObjectContext"));
}

static id safeValue(id object, NSString *key) {
    if (!object || key.length == 0) return nil;
    @try {
        return [object valueForKey:key];
    } @catch (__unused NSException *exception) {
        return nil;
    }
}

static void safeSetValue(id object, NSString *key, id value) {
    if (!object || key.length == 0 || !value) return;
    @try {
        [object setValue:value forKey:key];
    } @catch (__unused NSException *exception) {
    }
}

static NSString *safeString(id object, NSString *key) {
    id value = safeValue(object, key);
    if (!value || value == (id)[NSNull null]) return @"";
    return [NSString stringWithFormat:@"%@", value];
}

static NSString *objectURI(id object) {
    if (![object isKindOfClass:[NSManagedObject class]]) return @"";
    NSURL *url = [[(NSManagedObject *)object objectID] URIRepresentation];
    return url.absoluteString ?: @"";
}

static NSArray *fetchObjects(NSManagedObjectContext *moc, NSString *entityName, NSPredicate *predicate, NSArray<NSSortDescriptor *> *sortDescriptors, NSError **outError) {
    NSFetchRequest *request = [NSFetchRequest fetchRequestWithEntityName:entityName];
    request.predicate = predicate;
    request.sortDescriptors = sortDescriptors;
    return [moc executeFetchRequest:request error:outError] ?: @[];
}

static BOOL saveContext(NSManagedObjectContext *moc, NSError **outError) {
    if (!moc.hasChanges) return YES;
    return [moc save:outError];
}

static NSDictionary *accountJSON(id account) {
    NSString *name = safeString(account, @"name");
    if (name.length == 0) name = safeString(account, @"accountName");
    return @{
        @"id": objectURI(account),
        @"name": name,
        @"identifier": safeString(account, @"identifier"),
        @"accountName": safeString(account, @"accountName")
    };
}

static NSDictionary *folderJSON(id folder) {
    id account = safeValue(folder, @"account") ?: safeValue(folder, @"owner");
    id parent = safeValue(folder, @"parent") ?: safeValue(folder, @"parentFolder");
    NSString *name = safeString(folder, @"title");
    if (name.length == 0 && [folder respondsToSelector:NSSelectorFromString(@"localizedTitle")]) {
        id localized = send0(folder, NSSelectorFromString(@"localizedTitle"));
        name = localized ? [NSString stringWithFormat:@"%@", localized] : @"";
    }
    return @{
        @"id": objectURI(folder),
        @"name": name,
        @"title": name,
        @"account": safeString(account, @"name"),
        @"parent": parent ? safeString(parent, @"title") : (id)[NSNull null],
        @"parentId": parent ? objectURI(parent) : (id)[NSNull null],
        @"shared": ([folder respondsToSelector:NSSelectorFromString(@"isShared")] ? @(bool0(folder, NSSelectorFromString(@"isShared"))) : @NO)
    };
}

static NSString *noteHTML(id note) {
    if ([note respondsToSelector:NSSelectorFromString(@"scriptingBody")]) {
        id body = send0(note, NSSelectorFromString(@"scriptingBody"));
        if (body) return [NSString stringWithFormat:@"%@", body];
    }
    if ([note respondsToSelector:NSSelectorFromString(@"htmlString")]) {
        id html = send0(note, NSSelectorFromString(@"htmlString"));
        if (html) return [NSString stringWithFormat:@"%@", html];
    }
    return @"";
}

static NSString *notePlainText(id note) {
    if ([note respondsToSelector:NSSelectorFromString(@"scriptingPlainText")]) {
        id plain = send0(note, NSSelectorFromString(@"scriptingPlainText"));
        if (plain) return [NSString stringWithFormat:@"%@", plain];
    }
    if ([note respondsToSelector:NSSelectorFromString(@"noteAsPlainText")]) {
        id plain = send0(note, NSSelectorFromString(@"noteAsPlainText"));
        if (plain) return [NSString stringWithFormat:@"%@", plain];
    }
    return @"";
}

static NSDictionary *noteJSON(id note, BOOL includeBody) {
    id folder = safeValue(note, @"folder");
    id account = safeValue(note, @"account") ?: ([note respondsToSelector:NSSelectorFromString(@"cloudAccount")] ? send0(note, NSSelectorFromString(@"cloudAccount")) : nil);
    NSString *title = safeString(note, @"title");
    NSMutableDictionary *json = [@{
        @"id": objectURI(note),
        @"name": title,
        @"title": title,
        @"folder": folder ? safeString(folder, @"title") : @"",
        @"folderId": folder ? objectURI(folder) : @"",
        @"account": account ? safeString(account, @"name") : @"",
        @"createdAt": [NSString stringWithFormat:@"%@", safeValue(note, @"creationDate") ?: @""],
        @"modifiedAt": [NSString stringWithFormat:@"%@", safeValue(note, @"modificationDate") ?: @""],
        @"shared": ([note respondsToSelector:NSSelectorFromString(@"isSharedViaICloud")] ? @(bool0(note, NSSelectorFromString(@"isSharedViaICloud"))) : @NO),
        @"passwordProtected": ([note respondsToSelector:NSSelectorFromString(@"isPasswordProtected")] ? @(bool0(note, NSSelectorFromString(@"isPasswordProtected"))) : @NO)
    } mutableCopy];
    if (includeBody) {
        json[@"body"] = noteHTML(note);
        json[@"html"] = json[@"body"];
        json[@"plaintext"] = notePlainText(note);
    }
    return json;
}

static NSDictionary *attachmentJSON(id attachment) {
    NSString *name = safeString(attachment, @"filename");
    if (name.length == 0) name = safeString(attachment, @"title");
    if (name.length == 0) name = safeString(attachment, @"fallbackTitle");
    id media = safeValue(attachment, @"media");
    if (name.length == 0) name = safeString(media, @"filename");
    id note = safeValue(attachment, @"note");
    return @{
        @"id": objectURI(attachment),
        @"noteId": note ? objectURI(note) : @"",
        @"name": name,
        @"title": safeString(attachment, @"title"),
        @"filename": safeString(attachment, @"filename").length > 0 ? safeString(attachment, @"filename") : safeString(media, @"filename"),
        @"contentIdentifier": ([attachment respondsToSelector:NSSelectorFromString(@"contentIdentifier")] ? [NSString stringWithFormat:@"%@", send0(attachment, NSSelectorFromString(@"contentIdentifier")) ?: @""] : @""),
        @"typeUTI": safeString(attachment, @"typeUTI"),
        @"createdAt": [NSString stringWithFormat:@"%@", safeValue(attachment, @"creationDate") ?: @""],
        @"modifiedAt": [NSString stringWithFormat:@"%@", safeValue(attachment, @"modificationDate") ?: @""]
    };
}

static id objectForURI(NSManagedObjectContext *moc, NSString *uri, NSError **outError) {
    NSURL *url = [NSURL URLWithString:uri ?: @""];
    if (!url) return nil;
    NSPersistentStoreCoordinator *psc = moc.persistentStoreCoordinator;
    NSManagedObjectID *objectID = [psc managedObjectIDForURIRepresentation:url];
    if (!objectID) return nil;
    return [moc existingObjectWithID:objectID error:outError];
}

static id firstAccount(NSManagedObjectContext *moc, NSString *name) {
    NSError *error = nil;
    NSArray *accounts = fetchObjects(moc, @"ICAccount", nil, @[[NSSortDescriptor sortDescriptorWithKey:@"name" ascending:YES]], &error);
    for (id account in accounts) {
        if ([safeValue(account, @"markedForDeletion") boolValue]) continue;
        if (name.length == 0 || [safeString(account, @"name") isEqualToString:name] || [safeString(account, @"accountName") isEqualToString:name]) {
            return account;
        }
    }
    return nil;
}

static id folderMatching(NSManagedObjectContext *moc, id account, NSString *folderNameOrID) {
    if ([folderNameOrID hasPrefix:@"x-coredata://"]) {
        NSError *error = nil;
        id folder = objectForURI(moc, folderNameOrID, &error);
        if (folder) return folder;
    }
    NSError *error = nil;
    NSArray *folders = fetchObjects(moc, @"ICFolder", nil, @[[NSSortDescriptor sortDescriptorWithKey:@"title" ascending:YES]], &error);
    for (id folder in folders) {
        if ([safeValue(folder, @"markedForDeletion") boolValue]) continue;
        id folderAccount = safeValue(folder, @"account") ?: safeValue(folder, @"owner");
        if (account && folderAccount && folderAccount != account) continue;
        NSString *title = safeString(folder, @"title");
        if ([title isEqualToString:folderNameOrID] || (folderNameOrID.length == 0 && [title isEqualToString:@"Notes"])) {
            return folder;
        }
    }
    return nil;
}

static id noteForID(NSManagedObjectContext *moc, NSString *noteID, NSError **outError) {
    id note = objectForURI(moc, noteID, outError);
    if (note) return note;
    NSArray *notes = fetchObjects(moc, @"ICNote", [NSPredicate predicateWithFormat:@"identifier == %@", noteID], nil, outError);
    return notes.firstObject;
}

static NSArray *attachmentsForNote(id note) {
    NSMutableArray *items = [NSMutableArray array];
    id attachments = safeValue(note, @"attachments");
    if ([attachments respondsToSelector:@selector(allObjects)]) {
        [items addObjectsFromArray:[attachments allObjects]];
    } else if ([attachments isKindOfClass:[NSArray class]]) {
        [items addObjectsFromArray:attachments];
    } else if ([attachments isKindOfClass:[NSSet class]]) {
        [items addObjectsFromArray:[attachments allObjects]];
    }
    return items;
}

static id attachmentForIDOrName(id note, NSString *attachmentID, NSString *name) {
    for (id attachment in attachmentsForNote(note)) {
        if (attachmentID.length > 0 && [objectURI(attachment) isEqualToString:attachmentID]) return attachment;
        if (name.length > 0) {
            NSDictionary *json = attachmentJSON(attachment);
            if ([json[@"name"] isEqualToString:name] || [json[@"filename"] isEqualToString:name] || [json[@"title"] isEqualToString:name]) return attachment;
        }
    }
    return nil;
}

static NSArray *classNamesContaining(NSArray<NSString *> *needles) {
    int count = objc_getClassList(NULL, 0);
    Class *classes = (Class *)calloc((size_t)count, sizeof(Class));
    objc_getClassList(classes, count);
    NSMutableArray *names = [NSMutableArray array];
    for (int i = 0; i < count; i++) {
        const char *rawName = class_getName(classes[i]);
        if (!rawName) continue;
        NSString *name = @(rawName);
        for (NSString *needle in needles) {
            if ([name rangeOfString:needle options:NSCaseInsensitiveSearch].location != NSNotFound) {
                [names addObject:name];
                break;
            }
        }
    }
    free(classes);
    return [names copy];
}

static NSArray *knownInterestingClasses(void) {
    NSArray *candidates = @[
        @"ICAccount",
        @"ICAttachment",
        @"ICAttachmentModel",
        @"ICCloudContext",
        @"ICFolder",
        @"ICMedia",
        @"ICNote",
        @"ICNoteContext",
        @"ICTextController"
    ];
    NSMutableArray *names = [NSMutableArray array];
    for (NSString *candidate in candidates) {
        if (NSClassFromString(candidate)) {
            [names addObject:candidate];
        }
    }
    return [names copy];
}

static NSArray *methodNamesForClassName(NSString *className) {
    Class cls = NSClassFromString(className);
    if (!cls) return @[];
    unsigned int count = 0;
    Method *methods = class_copyMethodList(cls, &count);
    NSMutableArray *names = [NSMutableArray array];
    for (unsigned int i = 0; i < count; i++) {
        NSString *name = NSStringFromSelector(method_getName(methods[i]));
        if (name) {
            [names addObject:name];
        }
    }
    free(methods);
    return [names copy];
}

static NSArray *stringArrayFromObjects(NSArray *objects) {
    NSMutableArray *strings = [NSMutableArray array];
    for (id object in objects) {
        if (!object) continue;
        NSString *string = [NSString stringWithFormat:@"%@", object];
        if (string) {
            [strings addObject:string];
        }
    }
    return [strings copy];
}

static NSArray *entitySummaries(NSManagedObjectContext *moc) {
    NSMutableArray *entities = [NSMutableArray array];
    NSManagedObjectModel *model = moc.persistentStoreCoordinator.managedObjectModel;
    for (NSEntityDescription *entity in model.entities) {
        NSMutableDictionary *entry = [@{@"name": entity.name ?: @"", @"className": entity.managedObjectClassName ?: @""} mutableCopy];
        entry[@"attributes"] = stringArrayFromObjects([entity.attributesByName allKeys] ?: @[]);
        entry[@"relationships"] = stringArrayFromObjects([entity.relationshipsByName allKeys] ?: @[]);
        [entities addObject:entry];
    }
    return entities;
}

static void performProbe(void) {
    NSLog(@"apple-cli private probe: context");
    NSManagedObjectContext *moc = notesManagedObjectContext();
    if (!moc) {
        writeResult(@{@"status": @"error", @"stage": @"context", @"error": @"could not get Notes managed object context"});
        return;
    }

    NSLog(@"apple-cli private probe: class names");
    NSArray *interestingClasses = knownInterestingClasses();
    NSLog(@"apple-cli private probe: methods");
    NSMutableDictionary *methods = [NSMutableDictionary dictionary];
    for (NSString *name in interestingClasses) {
        if ([name isEqualToString:@"ICNote"] ||
            [name isEqualToString:@"ICFolder"] ||
            [name isEqualToString:@"ICAccount"] ||
            [name isEqualToString:@"ICAttachment"] ||
            [name isEqualToString:@"ICMedia"] ||
            [name isEqualToString:@"ICAttachmentModel"]) {
            methods[name] = methodNamesForClassName(name);
        }
    }

    NSLog(@"apple-cli private probe: entities");
    NSArray *entities = entitySummaries(moc);
    NSLog(@"apple-cli private probe: write");
    writeResult(@{
        @"status": @"ok",
        @"operation": @"probe",
        @"entities": entities,
        @"classes": interestingClasses,
        @"methods": methods
    });
}

static void writeOK(id result) {
    writeResult(@{@"status": @"ok", @"result": result ?: @{}});
}

static void writeError(NSString *stage, NSString *message) {
    writeResult(@{@"status": @"error", @"stage": stage ?: @"operation", @"error": message ?: @"unknown error"});
}

static NSManagedObjectContext *operationContext(void) {
    NSManagedObjectContext *moc = notesManagedObjectContext();
    if (!moc) {
        writeError(@"context", @"could not get Notes managed object context");
        return nil;
    }
    return moc;
}

static void performAccountsList(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    NSArray *accounts = fetchObjects(moc, @"ICAccount", nil, @[[NSSortDescriptor sortDescriptorWithKey:@"name" ascending:YES]], &error);
    if (!accounts && error) {
        writeError(@"accounts.list", [error description]);
        return;
    }
    NSMutableArray *items = [NSMutableArray array];
    for (id account in accounts) {
        if ([safeValue(account, @"markedForDeletion") boolValue]) continue;
        [items addObject:accountJSON(account)];
    }
    writeOK(items);
}

static void performFoldersList(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSString *accountName = params[@"account"] ?: @"";
    id account = firstAccount(moc, accountName);
    NSError *error = nil;
    NSArray *folders = fetchObjects(moc, @"ICFolder", nil, @[[NSSortDescriptor sortDescriptorWithKey:@"title" ascending:YES]], &error);
    if (!folders && error) {
        writeError(@"folders.list", [error description]);
        return;
    }
    NSMutableArray *items = [NSMutableArray array];
    for (id folder in folders) {
        if ([safeValue(folder, @"markedForDeletion") boolValue]) continue;
        id folderAccount = safeValue(folder, @"account") ?: safeValue(folder, @"owner");
        if (account && folderAccount && folderAccount != account) continue;
        [items addObject:folderJSON(folder)];
    }
    writeOK(items);
}

static void performFoldersCreate(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSString *name = params[@"name"] ?: @"";
    if (name.length == 0) {
        writeError(@"folders.create", @"name is required");
        return;
    }
    id account = firstAccount(moc, params[@"account"] ?: @"");
    if (!account) {
        writeError(@"folders.create", @"account not found");
        return;
    }
    id existing = folderMatching(moc, account, name);
    if (existing) {
        writeOK(folderJSON(existing));
        return;
    }
    id folder = [NSEntityDescription insertNewObjectForEntityForName:@"ICFolder" inManagedObjectContext:moc];
    safeSetValue(folder, @"title", name);
    safeSetValue(folder, @"account", account);
    safeSetValue(folder, @"owner", account);
    NSString *parentName = params[@"parent"] ?: @"";
    id parent = parentName.length > 0 ? folderMatching(moc, account, parentName) : nil;
    if (parentName.length > 0 && !parent) {
        writeError(@"folders.create", @"parent folder not found");
        [moc deleteObject:folder];
        return;
    }
    if (parent) {
        safeSetValue(folder, @"parent", parent);
    }
    safeSetValue(folder, @"dateForLastTitleModification", [NSDate date]);
    NSError *error = nil;
    if (!saveContext(moc, &error)) {
        writeError(@"folders.create", [error description]);
        return;
    }
    writeOK(folderJSON(folder));
}

static void performFoldersRename(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    id account = firstAccount(moc, params[@"account"] ?: @"");
    id folder = folderMatching(moc, account, params[@"name"] ?: @"");
    NSString *newName = params[@"newName"] ?: params[@"new_name"] ?: @"";
    if (!folder || newName.length == 0) {
        writeError(@"folders.rename", @"folder and newName are required");
        return;
    }
    safeSetValue(folder, @"title", newName);
    NSError *error = nil;
    if (!saveContext(moc, &error)) {
        writeError(@"folders.rename", [error description]);
        return;
    }
    writeOK(folderJSON(folder));
}

static void performFoldersDelete(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    id account = firstAccount(moc, params[@"account"] ?: @"");
    id folder = folderMatching(moc, account, params[@"name"] ?: @"");
    if (!folder) {
        writeError(@"folders.delete", @"folder not found");
        return;
    }
    [moc deleteObject:folder];
    NSError *error = nil;
    if (!saveContext(moc, &error)) {
        writeError(@"folders.delete", [error description]);
        return;
    }
    writeOK(@{@"status": @"ok"});
}

static NSArray *notesMatchingParams(NSManagedObjectContext *moc, NSDictionary *params, BOOL includeSharedOnly) {
    NSString *accountName = params[@"account"] ?: @"";
    NSString *folderName = params[@"folder"] ?: @"";
    id account = firstAccount(moc, accountName);
    id folder = folderName.length > 0 ? folderMatching(moc, account, folderName) : nil;
    NSError *error = nil;
    NSArray *notes = fetchObjects(moc, @"ICNote", nil, @[[NSSortDescriptor sortDescriptorWithKey:@"modificationDate" ascending:NO]], &error);
    NSMutableArray *items = [NSMutableArray array];
    for (id note in notes) {
        if ([safeValue(note, @"markedForDeletion") boolValue]) continue;
        id noteAccount = safeValue(note, @"account");
        id noteFolder = safeValue(note, @"folder");
        if (account && noteAccount && noteAccount != account) continue;
        if (folder && noteFolder != folder) continue;
        if (includeSharedOnly && (! [note respondsToSelector:NSSelectorFromString(@"isSharedViaICloud")] || !bool0(note, NSSelectorFromString(@"isSharedViaICloud")))) continue;
        [items addObject:note];
    }
    return items;
}

static void performNotesList(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    BOOL sharedOnly = [params[@"sharedOnly"] boolValue];
    NSMutableArray *items = [NSMutableArray array];
    for (id note in notesMatchingParams(moc, params, sharedOnly)) {
        [items addObject:noteJSON(note, NO)];
    }
    writeOK(items);
}

static void performNotesGet(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    id note = noteForID(moc, params[@"id"] ?: params[@"noteId"] ?: @"", &error);
    if (!note) {
        writeError(@"notes.get", @"note not found");
        return;
    }
    writeOK(noteJSON(note, YES));
}

static void addAttachmentsToNote(id note, NSArray *paths) {
    for (NSString *path in paths) {
        if (![path isKindOfClass:[NSString class]] || path.length == 0) continue;
        NSURL *url = [NSURL fileURLWithPath:path];
        NSString *filename = path.lastPathComponent;
        if ([note respondsToSelector:NSSelectorFromString(@"addAttachmentWithFileURL:filename:")]) {
            send2(note, NSSelectorFromString(@"addAttachmentWithFileURL:filename:"), url, filename);
        } else if ([note respondsToSelector:NSSelectorFromString(@"addAttachmentWithFileURL:")]) {
            send1(note, NSSelectorFromString(@"addAttachmentWithFileURL:"), url);
        } else {
            NSData *data = [NSData dataWithContentsOfURL:url];
            if (data && [note respondsToSelector:NSSelectorFromString(@"addAttachmentWithData:filename:")]) {
                send2(note, NSSelectorFromString(@"addAttachmentWithData:filename:"), data, filename);
            } else {
                NSLog(@"apple-cli private attachment add failed for %@", path);
            }
        }
    }
}

static void performNotesCreate(NSDictionary *params) {
    NSLog(@"apple-cli private op notes.create: context");
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSLog(@"apple-cli private op notes.create: resolve account/folder");
    id account = firstAccount(moc, params[@"account"] ?: @"");
    if (!account) {
        writeError(@"notes.create", @"account not found");
        return;
    }
    NSString *folderName = params[@"folder"] ?: @"Notes";
    id folder = folderMatching(moc, account, folderName);
    if (!folder) {
        writeError(@"notes.create", @"folder not found");
        return;
    }
    NSLog(@"apple-cli private op notes.create: insert note");
    NSString *title = params[@"title"] ?: params[@"name"] ?: @"Untitled";
    NSString *body = params[@"html"] ?: params[@"body"] ?: @"";
    id note = [NSEntityDescription insertNewObjectForEntityForName:@"ICNote" inManagedObjectContext:moc];
    id noteData = [NSEntityDescription insertNewObjectForEntityForName:@"ICNoteData" inManagedObjectContext:moc];
    safeSetValue(note, @"account", account);
    safeSetValue(note, @"folder", folder);
    safeSetValue(noteData, @"note", note);
    safeSetValue(note, @"noteData", noteData);
    safeSetValue(noteData, @"data", [NSData data]);
    safeSetValue(note, @"title", title);
    safeSetValue(note, @"creationDate", [NSDate date]);
    safeSetValue(note, @"modificationDate", [NSDate date]);
    if ([note respondsToSelector:NSSelectorFromString(@"setScriptingBody:")]) {
        NSLog(@"apple-cli private op notes.create: set body");
        ((void (*)(id, SEL, id))objc_msgSend)(note, NSSelectorFromString(@"setScriptingBody:"), body);
    }
    NSLog(@"apple-cli private op notes.create: add attachments");
    addAttachmentsToNote(note, params[@"attachments"] ?: @[]);
    if ([note respondsToSelector:NSSelectorFromString(@"regenerateTitle:snippet:")]) {
        ((void (*)(id, SEL, BOOL, BOOL))objc_msgSend)(note, NSSelectorFromString(@"regenerateTitle:snippet:"), YES, YES);
    }
    NSError *error = nil;
    NSLog(@"apple-cli private op notes.create: save");
    if (!saveContext(moc, &error)) {
        writeError(@"notes.create", [error description]);
        return;
    }
    NSLog(@"apple-cli private op notes.create: write result");
    writeOK(noteJSON(note, YES));
}

static void performNotesUpdate(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    id note = noteForID(moc, params[@"id"] ?: params[@"noteId"] ?: @"", &error);
    if (!note) {
        writeError(@"notes.update", @"note not found");
        return;
    }
    NSString *title = params[@"title"] ?: params[@"name"] ?: @"";
    NSString *body = params[@"html"] ?: params[@"body"] ?: @"";
    if (title.length > 0) safeSetValue(note, @"title", title);
    if (body.length > 0 && [note respondsToSelector:NSSelectorFromString(@"setScriptingBody:")]) {
        ((void (*)(id, SEL, id))objc_msgSend)(note, NSSelectorFromString(@"setScriptingBody:"), body);
    }
    addAttachmentsToNote(note, params[@"attachments"] ?: @[]);
    safeSetValue(note, @"modificationDate", [NSDate date]);
    if (!saveContext(moc, &error)) {
        writeError(@"notes.update", [error description]);
        return;
    }
    writeOK(noteJSON(note, YES));
}

static void performNotesDelete(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    id note = noteForID(moc, params[@"id"] ?: params[@"noteId"] ?: @"", &error);
    if (!note) {
        writeError(@"notes.delete", @"note not found");
        return;
    }
    [moc deleteObject:note];
    if (!saveContext(moc, &error)) {
        writeError(@"notes.delete", [error description]);
        return;
    }
    writeOK(@{@"status": @"ok"});
}

static void performNotesMove(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    id note = noteForID(moc, params[@"id"] ?: params[@"noteId"] ?: @"", &error);
    if (!note) {
        writeError(@"notes.move", @"note not found");
        return;
    }
    id account = firstAccount(moc, params[@"account"] ?: @"");
    id folder = folderMatching(moc, account, params[@"folder"] ?: @"");
    if (!folder) {
        writeError(@"notes.move", @"folder not found");
        return;
    }
    if ([note respondsToSelector:NSSelectorFromString(@"setFolder:")]) {
        ((void (*)(id, SEL, id))objc_msgSend)(note, NSSelectorFromString(@"setFolder:"), folder);
    } else {
        safeSetValue(note, @"folder", folder);
    }
    if (!saveContext(moc, &error)) {
        writeError(@"notes.move", [error description]);
        return;
    }
    writeOK(noteJSON(note, NO));
}

static void performNotesSearch(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSString *query = params[@"query"] ?: @"";
    NSUInteger limit = [params[@"limit"] unsignedIntegerValue];
    NSMutableArray *items = [NSMutableArray array];
    for (id note in notesMatchingParams(moc, params, NO)) {
        NSString *haystack = [[noteJSON(note, YES) description] lowercaseString];
        if (query.length == 0 || [haystack rangeOfString:[query lowercaseString]].location != NSNotFound) {
            [items addObject:noteJSON(note, NO)];
            if (limit > 0 && items.count >= limit) break;
        }
    }
    writeOK(items);
}

static void performAttachmentsList(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    id note = noteForID(moc, params[@"noteId"] ?: params[@"id"] ?: @"", &error);
    if (!note) {
        writeError(@"attachments.list", @"note not found");
        return;
    }
    NSMutableArray *items = [NSMutableArray array];
    for (id attachment in attachmentsForNote(note)) {
        [items addObject:attachmentJSON(attachment)];
    }
    writeOK(items);
}

static NSURL *attachmentFileURL(id attachment) {
    if ([attachment respondsToSelector:NSSelectorFromString(@"fileURL")]) {
        id url = send0(attachment, NSSelectorFromString(@"fileURL"));
        if ([url isKindOfClass:[NSURL class]]) return url;
    }
    id media = safeValue(attachment, @"media");
    if (media && [media respondsToSelector:NSSelectorFromString(@"mediaURL")]) {
        id url = send0(media, NSSelectorFromString(@"mediaURL"));
        if ([url isKindOfClass:[NSURL class]]) return url;
    }
    return nil;
}

static NSData *attachmentData(id attachment) {
    NSString *typeUTI = safeString(attachment, @"typeUTI");
    if (typeUTI.length == 0) typeUTI = @"public.data";
    if ([attachment respondsToSelector:NSSelectorFromString(@"dataForTypeIdentifier:")]) {
        id data = send1(attachment, NSSelectorFromString(@"dataForTypeIdentifier:"), typeUTI);
        if ([data isKindOfClass:[NSData class]]) return data;
    }
    id model = nil;
    if ([attachment respondsToSelector:NSSelectorFromString(@"attachmentModel")]) {
        model = send0(attachment, NSSelectorFromString(@"attachmentModel"));
    }
    if (model && [model respondsToSelector:NSSelectorFromString(@"dataForTypeIdentifier:")]) {
        id data = send1(model, NSSelectorFromString(@"dataForTypeIdentifier:"), typeUTI);
        if ([data isKindOfClass:[NSData class]]) return data;
    }
    if (model && [model respondsToSelector:NSSelectorFromString(@"dataForQuickLook")]) {
        id data = send0(model, NSSelectorFromString(@"dataForQuickLook"));
        if ([data isKindOfClass:[NSData class]]) return data;
    }
    NSURL *url = attachmentFileURL(attachment);
    if (url) return [NSData dataWithContentsOfURL:url];
    return nil;
}

static void performAttachmentsSave(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    id note = noteForID(moc, params[@"noteId"] ?: params[@"id"] ?: @"", &error);
    if (!note) {
        writeError(@"attachments.save", @"note not found");
        return;
    }
    id attachment = attachmentForIDOrName(note, params[@"attachmentId"] ?: params[@"attachment_id"] ?: @"", params[@"name"] ?: @"");
    if (!attachment) {
        writeError(@"attachments.save", @"attachment not found");
        return;
    }
    NSURL *source = attachmentFileURL(attachment);
    if (!source) {
        NSData *data = attachmentData(attachment);
        if (!data) {
            writeError(@"attachments.save", @"attachment has no local file URL or readable data");
            return;
        }
        NSDictionary *metadata = attachmentJSON(attachment);
        writeOK(@{
            @"name": metadata[@"name"] ?: @"attachment",
            @"contentType": metadata[@"typeUTI"] ?: @"application/octet-stream",
            @"size": @(data.length),
            @"dataBase64": [data base64EncodedStringWithOptions:0]
        });
        return;
    }
    NSString *output = params[@"output"] ?: params[@"outputDir"] ?: params[@"output_dir"] ?: @"";
    if (output.length == 0) {
        writeOK(@{@"path": source.path ?: @"", @"sourcePath": source.path ?: @""});
        return;
    }
    NSString *name = attachmentJSON(attachment)[@"name"];
    if (name.length == 0) name = source.lastPathComponent;
    NSString *destination = [output stringByAppendingPathComponent:name];
    [[NSFileManager defaultManager] createDirectoryAtPath:output withIntermediateDirectories:YES attributes:nil error:nil];
    if (![[NSFileManager defaultManager] copyItemAtURL:source toURL:[NSURL fileURLWithPath:destination] error:&error]) {
        if ([[NSFileManager defaultManager] fileExistsAtPath:destination]) {
            [[NSFileManager defaultManager] removeItemAtPath:destination error:nil];
            error = nil;
            [[NSFileManager defaultManager] copyItemAtURL:source toURL:[NSURL fileURLWithPath:destination] error:&error];
        }
    }
    if (error) {
        writeOK(@{@"path": source.path ?: @"", @"sourcePath": source.path ?: @"", @"copyWarning": [error description]});
        return;
    }
    writeOK(@{@"path": destination});
}

static void performAttachmentsDelete(NSDictionary *params) {
    NSManagedObjectContext *moc = operationContext();
    if (!moc) return;
    NSError *error = nil;
    id note = noteForID(moc, params[@"noteId"] ?: params[@"id"] ?: @"", &error);
    if (!note) {
        writeError(@"attachments.delete", @"note not found");
        return;
    }
    id attachment = attachmentForIDOrName(note, params[@"attachmentId"] ?: params[@"attachment_id"] ?: @"", params[@"name"] ?: @"");
    if (!attachment) {
        writeError(@"attachments.delete", @"attachment not found");
        return;
    }
    if ([attachment respondsToSelector:NSSelectorFromString(@"deleteFromLocalDatabase")]) {
        ((void (*)(id, SEL))objc_msgSend)(attachment, NSSelectorFromString(@"deleteFromLocalDatabase"));
    } else {
        [moc deleteObject:attachment];
    }
    if (!saveContext(moc, &error)) {
        writeError(@"attachments.delete", [error description]);
        return;
    }
    writeOK(@{@"status": @"ok"});
}

static void performPrivateOperation(void) {
    @autoreleasepool {
        @try {
            NSLog(@"apple-cli private op: read request");
            NSDictionary *request = readRequest();
            NSString *op = request[@"op"] ?: @"probe";
            NSDictionary *params = request[@"params"] ?: @{};
            NSLog(@"apple-cli private op: dispatch %@", op);
            if ([op isEqualToString:@"probe"]) {
                performProbe();
                return;
            }
            if ([op isEqualToString:@"error"]) {
                writeResult(@{@"status": @"error", @"stage": @"request", @"error": request[@"error"] ?: @"invalid request"});
                return;
            }
            if ([op isEqualToString:@"accounts.list"]) { performAccountsList(params); return; }
            if ([op isEqualToString:@"folders.list"]) { performFoldersList(params); return; }
            if ([op isEqualToString:@"folders.create"]) { performFoldersCreate(params); return; }
            if ([op isEqualToString:@"folders.rename"]) { performFoldersRename(params); return; }
            if ([op isEqualToString:@"folders.delete"]) { performFoldersDelete(params); return; }
            if ([op isEqualToString:@"notes.list"]) { performNotesList(params); return; }
            if ([op isEqualToString:@"notes.get"]) { performNotesGet(params); return; }
            if ([op isEqualToString:@"notes.create"]) { performNotesCreate(params); return; }
            if ([op isEqualToString:@"notes.update"]) { performNotesUpdate(params); return; }
            if ([op isEqualToString:@"notes.delete"]) { performNotesDelete(params); return; }
            if ([op isEqualToString:@"notes.move"]) { performNotesMove(params); return; }
            if ([op isEqualToString:@"notes.search"]) { performNotesSearch(params); return; }
            if ([op isEqualToString:@"attachments.list"]) { performAttachmentsList(params); return; }
            if ([op isEqualToString:@"attachments.save"]) { performAttachmentsSave(params); return; }
            if ([op isEqualToString:@"attachments.delete"]) { performAttachmentsDelete(params); return; }
            writeResult(@{@"status": @"error", @"stage": @"dispatch", @"error": [NSString stringWithFormat:@"unsupported private operation %@", op]});
        } @catch (NSException *exception) {
            writeResult(@{@"status": @"error", @"stage": @"exception", @"error": [NSString stringWithFormat:@"%@: %@", exception.name, exception.reason]});
        }
    }
}

static BOOL shouldRunPrivateOperationOnMainQueue(void) {
    NSDictionary *request = readRequest();
    NSString *op = request[@"op"] ?: @"probe";
    NSDictionary *params = request[@"params"] ?: @{};
    NSArray *attachments = params[@"attachments"] ?: @[];
    BOOL addsAttachments = [attachments isKindOfClass:[NSArray class]] && attachments.count > 0;
    if (([op isEqualToString:@"notes.create"] || [op isEqualToString:@"notes.update"]) && addsAttachments) {
        return NO;
    }
    return YES;
}

__attribute__((constructor))
static void AppleCLINotesPrivateInjected(void) {
    NSLog(@"apple-cli notes generic private helper loaded");
    dispatch_queue_t queue = shouldRunPrivateOperationOnMainQueue()
        ? dispatch_get_main_queue()
        : dispatch_get_global_queue(QOS_CLASS_UTILITY, 0);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC), queue, ^{
        performPrivateOperation();
    });
}
