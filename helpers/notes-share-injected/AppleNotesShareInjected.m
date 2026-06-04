#import <Foundation/Foundation.h>
#import <CoreData/CoreData.h>
#import <CloudKit/CloudKit.h>
#import <objc/message.h>
#import <dlfcn.h>

static id send0(id obj, SEL sel) {
    return ((id (*)(id, SEL))objc_msgSend)(obj, sel);
}

static id send1(id obj, SEL sel, id a) {
    return ((id (*)(id, SEL, id))objc_msgSend)(obj, sel, a);
}

static BOOL bool0(id obj, SEL sel) {
    return ((BOOL (*)(id, SEL))objc_msgSend)(obj, sel);
}

static long long q0(id obj, SEL sel) {
    return ((long long (*)(id, SEL))objc_msgSend)(obj, sel);
}

static void void1(id obj, SEL sel, id a) {
    ((void (*)(id, SEL, id))objc_msgSend)(obj, sel, a);
}

static void voidq(id obj, SEL sel, long long q) {
    ((void (*)(id, SEL, long long))objc_msgSend)(obj, sel, q);
}

static NSString *defaultResultPath(void) {
    NSString *dir = [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/apple-cli"];
    [[NSFileManager defaultManager] createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:nil error:nil];
    return [dir stringByAppendingPathComponent:@"notes-share-result.json"];
}

static void writeResult(NSDictionary *result) {
    NSString *path = [[[NSProcessInfo processInfo] environment] objectForKey:@"APPLE_CLI_NOTES_SHARE_RESULT"];
    if (path.length == 0) {
        path = defaultResultPath();
    }
    NSMutableDictionary *payload = [result mutableCopy];
    payload[@"result_path"] = path;
    NSError *jsonError = nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:payload options:NSJSONWritingPrettyPrinted error:&jsonError];
    if (!data) {
        NSLog(@"apple-cli notes share result JSON failed: %@", jsonError);
        return;
    }
    NSError *writeError = nil;
    BOOL ok = [data writeToFile:path options:NSDataWritingAtomic error:&writeError];
    if (!ok) {
        NSLog(@"apple-cli notes share result write failed path=%@ error=%@", path, writeError);
    } else {
        NSLog(@"apple-cli notes share wrote result to %@", path);
    }
}

static id loadNote(NSString *uriString, NSError **outError) {
    Class noteContextClass = NSClassFromString(@"ICNoteContext");
    if (!noteContextClass) {
        if (outError) *outError = [NSError errorWithDomain:@"AppleCLIInjectedShare" code:10 userInfo:@{NSLocalizedDescriptionKey: @"ICNoteContext not found"}];
        return nil;
    }
    if ([noteContextClass respondsToSelector:NSSelectorFromString(@"startSharedContextWithOptions:")]) {
        ((void (*)(Class, SEL, unsigned long long))objc_msgSend)(noteContextClass, NSSelectorFromString(@"startSharedContextWithOptions:"), 0);
    }
    id noteContext = ((id (*)(Class, SEL))objc_msgSend)(noteContextClass, NSSelectorFromString(@"sharedContext"));
    id moc = send0(noteContext, NSSelectorFromString(@"managedObjectContext"));
    id psc = send0(moc, @selector(persistentStoreCoordinator));
    NSURL *url = [NSURL URLWithString:uriString];
    id objectID = ((id (*)(id, SEL, id))objc_msgSend)(psc, @selector(managedObjectIDForURIRepresentation:), url);
    if (!objectID) {
        if (outError) *outError = [NSError errorWithDomain:@"AppleCLIInjectedShare" code:11 userInfo:@{NSLocalizedDescriptionKey: [NSString stringWithFormat:@"Could not resolve note URI: %@", uriString]}];
        return nil;
    }
    NSError *error = nil;
    id note = ((id (*)(id, SEL, id, NSError **))objc_msgSend)(moc, @selector(existingObjectWithID:error:), objectID, &error);
    if (!note && outError) *outError = error;
    return note;
}

static id containerForNote(id controller, id note) {
    id account = send0(note, NSSelectorFromString(@"cloudAccount"));
    id accountID = send0(account, NSSelectorFromString(@"identifier"));
    if (controller && accountID && [controller respondsToSelector:NSSelectorFromString(@"containerForAccountID:")]) {
        id container = send1(controller, NSSelectorFromString(@"containerForAccountID:"), accountID);
        if (container) return container;
    }
    Class ckContainerClass = NSClassFromString(@"CKContainer");
    return ((id (*)(Class, SEL, id))objc_msgSend)(ckContainerClass, NSSelectorFromString(@"containerWithIdentifier:"), @"com.apple.notes");
}

static void performShare(void) {
    @autoreleasepool {
        @try {
            dlopen("/System/Library/PrivateFrameworks/NotesShared.framework/NotesShared", RTLD_NOW);
            dlopen("/System/Library/PrivateFrameworks/NotesUI.framework/NotesUI", RTLD_NOW);
            dlopen("/System/Library/Frameworks/CloudKit.framework/CloudKit", RTLD_NOW);

            NSDictionary *env = [[NSProcessInfo processInfo] environment];
            NSString *noteURI = env[@"APPLE_CLI_NOTES_SHARE_NOTE_ID"];
            NSString *email = env[@"APPLE_CLI_NOTES_SHARE_EMAIL"];
            if (noteURI.length == 0 || email.length == 0) {
                writeResult(@{@"status": @"error", @"error": @"APPLE_CLI_NOTES_SHARE_NOTE_ID and APPLE_CLI_NOTES_SHARE_EMAIL are required"});
                return;
            }

            NSError *loadError = nil;
            id note = loadNote(noteURI, &loadError);
            if (!note) {
                writeResult(@{@"status": @"error", @"stage": @"load-note", @"error": [NSString stringWithFormat:@"%@", loadError]});
                return;
            }

            NSString *title = [NSString stringWithFormat:@"%@", send0(note, NSSelectorFromString(@"title")) ?: @""];
            BOOL canShare = bool0(note, NSSelectorFromString(@"canBeSharedViaICloud"));
            BOOL alreadyShared = bool0(note, NSSelectorFromString(@"isSharedViaICloud"));
            if (!canShare) {
                writeResult(@{@"status": @"error", @"stage": @"preflight", @"note": title, @"error": @"note cannot be shared via iCloud"});
                return;
            }

            Class collabClass = NSClassFromString(@"ICCollaborationController");
            id controller = ((id (*)(Class, SEL))objc_msgSend)(collabClass, NSSelectorFromString(@"sharedInstance"));
            id account = send0(note, NSSelectorFromString(@"cloudAccount"));
            id accountID = send0(account, NSSelectorFromString(@"identifier"));
            id container = containerForNote(controller, note);
            if (!container) {
                writeResult(@{@"status": @"error", @"stage": @"container", @"note": title, @"error": @"could not get Notes CloudKit container"});
                return;
            }

            dispatch_semaphore_t participantSema = dispatch_semaphore_create(0);
            __block id participant = nil;
            __block NSError *participantError = nil;
            ((void (*)(id, SEL, id, id))objc_msgSend)(container, NSSelectorFromString(@"fetchShareParticipantWithEmailAddress:completionHandler:"), email, ^(id p, NSError *error) {
                participant = p;
                participantError = error;
                dispatch_semaphore_signal(participantSema);
            });
            long participantWait = dispatch_semaphore_wait(participantSema, dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC));
            if (participantWait != 0 || !participant) {
                writeResult(@{@"status": @"error", @"stage": @"participant", @"note": title, @"error": [NSString stringWithFormat:@"%@", participantError ?: @"timed out fetching participant"]});
                return;
            }
            if ([participant respondsToSelector:NSSelectorFromString(@"setPermission:")]) {
                voidq(participant, NSSelectorFromString(@"setPermission:"), 3);
            }

            id rootRecordID = send0(note, NSSelectorFromString(@"recordID"));
            id database = send0(container, NSSelectorFromString(@"privateCloudDatabase"));
            if (!rootRecordID || !database) {
                writeResult(@{@"status": @"error", @"stage": @"root-record", @"note": title, @"error": @"could not get root record ID or private database"});
                return;
            }

            dispatch_semaphore_t fetchSema = dispatch_semaphore_create(0);
            __block CKRecord *rootRecord = nil;
            __block NSError *fetchError = nil;
            ((void (*)(id, SEL, id, id))objc_msgSend)(database, NSSelectorFromString(@"fetchRecordWithID:completionHandler:"), rootRecordID, ^(CKRecord *record, NSError *error) {
                rootRecord = record;
                fetchError = error;
                dispatch_semaphore_signal(fetchSema);
            });
            long fetchWait = dispatch_semaphore_wait(fetchSema, dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC));
            if (fetchWait != 0 || !rootRecord) {
                writeResult(@{@"status": @"error", @"stage": @"fetch-root-record", @"note": title, @"error": [NSString stringWithFormat:@"%@", fetchError ?: @"timed out fetching root record"]});
                return;
            }

            CKShare *share = [[CKShare alloc] initWithRootRecord:rootRecord];
            [share setObject:title forKeyedSubscript:@"cloudkit.title"];
            [share setObject:@"Shared Note" forKeyedSubscript:@"cloudkit.subtitle"];
            [share setObject:@"com.apple.notes.note" forKeyedSubscript:@"cloudkit.type"];
            share.publicPermission = CKShareParticipantPermissionNone;
            [share addParticipant:(CKShareParticipant *)participant];

            dispatch_semaphore_t saveSema = dispatch_semaphore_create(0);
            __block NSArray<CKRecord *> *savedRecords = nil;
            __block NSError *saveError = nil;
            CKModifyRecordsOperation *operation = [[CKModifyRecordsOperation alloc] initWithRecordsToSave:@[rootRecord, share] recordIDsToDelete:nil];
            operation.savePolicy = CKRecordSaveAllKeys;
            operation.modifyRecordsCompletionBlock = ^(NSArray<CKRecord *> *records, NSArray<CKRecordID *> *deletedRecordIDs, NSError *error) {
                savedRecords = records;
                saveError = error;
                dispatch_semaphore_signal(saveSema);
            };
            [database addOperation:operation];

            long saveWait = dispatch_semaphore_wait(saveSema, dispatch_time(DISPATCH_TIME_NOW, 120 * NSEC_PER_SEC));
            if (saveWait != 0 || saveError) {
                writeResult(@{@"status": @"error", @"stage": @"save", @"note": title, @"error": [NSString stringWithFormat:@"%@", saveError ?: @"timed out saving share"]});
                return;
            }

            for (CKRecord *record in savedRecords) {
                if ([record isKindOfClass:[CKShare class]]) {
                    share = (CKShare *)record;
                }
            }
            if ([note respondsToSelector:NSSelectorFromString(@"setServerShare:")]) {
                void1(note, NSSelectorFromString(@"setServerShare:"), share);
            }
            id moc = nil;
            if ([note respondsToSelector:NSSelectorFromString(@"managedObjectContext")]) {
                moc = send0(note, NSSelectorFromString(@"managedObjectContext"));
            }
            NSError *mocSaveError = nil;
            if (moc && [moc respondsToSelector:@selector(save:)]) {
                ((BOOL (*)(id, SEL, NSError **))objc_msgSend)(moc, @selector(save:), &mocSaveError);
            }
            if (mocSaveError) {
                writeResult(@{@"status": @"error", @"stage": @"context-save", @"note": title, @"error": [NSString stringWithFormat:@"%@", mocSaveError]});
                return;
            }

            NSString *shareURL = @"";
            if ([share respondsToSelector:NSSelectorFromString(@"URL")]) {
                id url = send0(share, NSSelectorFromString(@"URL"));
                shareURL = [NSString stringWithFormat:@"%@", url ?: @""];
            }
            BOOL participantCurrentUser = NO;
            long long participantAcceptanceStatus = -1;
            long long participantPermission = -1;
            NSUInteger savedParticipantCount = 0;
            if ([share respondsToSelector:NSSelectorFromString(@"participants")]) {
                NSArray *participants = send0(share, NSSelectorFromString(@"participants"));
                savedParticipantCount = participants.count;
            }
            if ([participant respondsToSelector:NSSelectorFromString(@"isCurrentUser")]) {
                participantCurrentUser = bool0(participant, NSSelectorFromString(@"isCurrentUser"));
            }
            if ([participant respondsToSelector:NSSelectorFromString(@"acceptanceStatus")]) {
                participantAcceptanceStatus = q0(participant, NSSelectorFromString(@"acceptanceStatus"));
            }
            if ([participant respondsToSelector:NSSelectorFromString(@"permission")]) {
                participantPermission = q0(participant, NSSelectorFromString(@"permission"));
            }
            BOOL sharedAfter = bool0(note, NSSelectorFromString(@"isSharedViaICloud"));
            if (!sharedAfter && savedParticipantCount <= 1) {
                writeResult(@{
                    @"status": @"error",
                    @"stage": @"verify-share",
                    @"note": title,
                    @"email": email,
                    @"saved_participant_count": @(savedParticipantCount),
                    @"shared_after": @(sharedAfter),
                    @"share_url": shareURL,
                    @"error": @"CloudKit saved an owner-only share; the invitee was not persisted"
                });
                return;
            }
            writeResult(@{
                @"status": @"ok",
                @"note": title,
                @"email": email,
                @"already_shared": @(alreadyShared),
                @"shared_after": @(sharedAfter),
                @"saved_participant_count": @(savedParticipantCount),
                @"participant_current_user": @(participantCurrentUser),
                @"participant_acceptance_status": @(participantAcceptanceStatus),
                @"participant_permission": @(participantPermission),
                @"share_url": shareURL
            });
        } @catch (NSException *exception) {
            writeResult(@{@"status": @"error", @"stage": @"exception", @"error": [NSString stringWithFormat:@"%@: %@", exception.name, exception.reason]});
        }
    }
}

__attribute__((constructor))
static void AppleCLINotesShareInjected(void) {
    NSLog(@"apple-cli notes injected share helper loaded");
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC), dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        performShare();
    });
}
