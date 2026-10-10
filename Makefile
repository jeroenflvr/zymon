# Toplevel Makefile for Xymon
BUILDTOPDIR=`pwd`
CLIENTONLY = yes
LOCALCLIENT = no

# configure settings for Xymon
#
# Toplevel dir
XYMONTOPDIR = /Users/jeroen

# Xymon settings follows
#
# Username running Xymon
XYMONUSER = jeroen
# Xymon server IP-address
XYMONHOSTIP = 192.168.0.100
# Large File Support settings
LFSDEF = -D_LARGEFILE_SOURCE -D_FILE_OFFSET_BITS=64
LIBRTDEF = 

include build/Makefile.Darwin


#
# Add local CFLAGS etc. settings here

include build/Makefile.rules

