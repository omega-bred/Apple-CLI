#import <Foundation/Foundation.h>
#import <CoreData/CoreData.h>
#import <objc/message.h>
#import <dlfcn.h>

static id send0(id obj, SEL sel) {
    return ((id (*)(id, SEL))objc_msgSend)(obj, sel);
}

static NSString *defaultResultPath(void) {
    NSString *dir = [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/apple-cli"];
    [[NSFileManager defaultManager] createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:nil error:nil];
    return [dir stringByAppendingPathComponent:@"notes-accept-result.json"];
}

static void writeResult(NSDictionary *result) {
    NSString *path = [[[NSProcessInfo processInfo] environment] objectForKey:@"APPLE_CLI_NOTES_ACCEPT_RESULT"];
    if (path.length == 0) {
        path = defaultResultPath();
    }
    NSMutableDictionary *payload = [result mutableCopy];
    payload[@"result_path"] = path;
    NSError *jsonError = nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:payload options:NSJSONWritingPrettyPrinted error:&jsonError];
    if (!data) {
        NSLog(@"apple-cli notes accept result JSON failed: %@", jsonError);
        return;
    }
    NSError *writeError = nil;
    BOOL ok = [data writeToFile:path options:NSDataWritingAtomic error:&writeError];
    if (!ok) {
        NSLog(@"apple-cli notes accept result write failed path=%@ error=%@", path, writeError);
    } else {
        NSLog(@"apple-cli notes accept wrote result to %@", path);
    }
}

static id notesManagedObjectContext(void) {
    Class noteContextClass = NSClassFromString(@"ICNoteContext");
    if (!noteContextClass) return nil;
    if ([noteContextClass respondsToSelector:NSSelectorFromString(@"startSharedContextWithOptions:")]) {
        ((void (*)(Class, SEL, unsigned long long))objc_msgSend)(noteContextClass, NSSelectorFromString(@"startSharedContextWithOptions:"), 0);
    }
    id noteContext = ((id (*)(Class, SEL))objc_msgSend)(noteContextClass, NSSelectorFromString(@"sharedContext"));
    return send0(noteContext, NSSelectorFromString(@"managedObjectContext"));
}

static void performAccept(void) {
    @autoreleasepool {
        @try {
            dlopen("/System/Library/PrivateFrameworks/NotesShared.framework/NotesShared", RTLD_NOW);
            dlopen("/System/Library/PrivateFrameworks/NotesUI.framework/NotesUI", RTLD_NOW);
            dlopen("/System/Library/Frameworks/CloudKit.framework/CloudKit", RTLD_NOW);

            NSString *urlString = [[[NSProcessInfo processInfo] environment] objectForKey:@"APPLE_CLI_NOTES_ACCEPT_URL"];
            if (urlString.length == 0) {
                writeResult(@{@"status": @"error", @"error": @"APPLE_CLI_NOTES_ACCEPT_URL is required"});
                return;
            }

            NSURL *url = [NSURL URLWithString:urlString];
            if (!url) {
                writeResult(@{@"status": @"error", @"error": @"invalid share URL"});
                return;
            }

            id moc = notesManagedObjectContext();
            if (!moc) {
                writeResult(@{@"status": @"error", @"stage": @"context", @"error": @"could not get Notes managed object context"});
                return;
            }

            Class collabClass = NSClassFromString(@"ICCollaborationController");
            id controller = ((id (*)(Class, SEL))objc_msgSend)(collabClass, NSSelectorFromString(@"sharedInstance"));
            if (!controller || ![controller respondsToSelector:NSSelectorFromString(@"fetchAndAcceptShareMetadataWithURL:managedObjectContext:alertBlock:showObjectBlock:")]) {
                writeResult(@{@"status": @"error", @"stage": @"controller", @"error": @"Notes collaboration accept selector is not available"});
                return;
            }

            dispatch_semaphore_t sema = dispatch_semaphore_create(0);
            __block NSString *alertText = @"";
            __block NSString *shownObject = @"";
            ((void (*)(id, SEL, id, id, id, id))objc_msgSend)(
                controller,
                NSSelectorFromString(@"fetchAndAcceptShareMetadataWithURL:managedObjectContext:alertBlock:showObjectBlock:"),
                url,
                moc,
                ^(id error) {
                    alertText = [NSString stringWithFormat:@"%@", error ?: @""];
                    dispatch_semaphore_signal(sema);
                },
                ^(id object) {
                    shownObject = [NSString stringWithFormat:@"%@", object ?: @""];
                    dispatch_semaphore_signal(sema);
                });

            long wait = dispatch_semaphore_wait(sema, dispatch_time(DISPATCH_TIME_NOW, 120 * NSEC_PER_SEC));
            if (wait != 0) {
                writeResult(@{@"status": @"error", @"stage": @"accept", @"url": urlString, @"error": @"timed out accepting share URL"});
                return;
            }
            if (alertText.length > 0) {
                writeResult(@{@"status": @"error", @"stage": @"accept", @"url": urlString, @"error": alertText});
                return;
            }
            writeResult(@{@"status": @"ok", @"url": urlString, @"object": shownObject});
        } @catch (NSException *exception) {
            writeResult(@{@"status": @"error", @"stage": @"exception", @"error": [NSString stringWithFormat:@"%@: %@", exception.name, exception.reason]});
        }
    }
}

__attribute__((constructor))
static void AppleCLINotesAcceptInjected(void) {
    NSLog(@"apple-cli notes injected accept helper loaded");
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC), dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        performAccept();
    });
}
